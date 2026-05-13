use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use tracing::{info, warn};
use v4l::FourCC;
use v4l::video::Output;

use crate::config::AppConfig;
use crate::live::LiveEvent;

pub fn run_output_loop(
    cfg: &AppConfig,
    dummy_frame: Vec<u8>,
    rx: Receiver<LiveEvent>,
    recycle_tx: Sender<Vec<u8>>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let mut out = open_device(cfg)?;
    let frame_interval = cfg.frame_interval();
    let live_timeout = Duration::from_millis(cfg.live_timeout_ms);

    let mut last_live_frame: Option<Vec<u8>> = None;
    let mut last_live_at: Option<Instant> = None;
    let mut live_active = false;

    info!("starting output loop");

    loop {
        if shutdown.load(Ordering::SeqCst) {
            info!("shutdown signal received; releasing loopback device");
            drop(out);
            return Ok(());
        }

        let cycle_start = Instant::now();

        while let Ok(event) = rx.try_recv() {
            match event {
                LiveEvent::Frame(frame) => {
                    if let Some(old) = last_live_frame.replace(frame) {
                        let _ = recycle_tx.try_send(old);
                    }
                    last_live_at = Some(Instant::now());
                }
                LiveEvent::StreamUp => {
                    live_active = true;
                    info!("stream state: live");
                }
                LiveEvent::StreamDown => {
                    live_active = false;
                    info!("stream state: dummy (disconnected)");
                }
            }
        }

        let should_use_live = live_active
            && last_live_at
                .map(|t| t.elapsed() <= live_timeout)
                .unwrap_or(false)
            && last_live_frame.is_some();

        let selected = if should_use_live {
            last_live_frame.as_ref().unwrap()
        } else {
            &dummy_frame
        };

        if let Err(err) = out.write_all(selected) {
            warn!(error = %err, "failed writing frame; attempting to reopen loopback device");
            thread::sleep(Duration::from_millis(100));
            out = open_device(cfg)?;
            continue;
        }

        let elapsed = cycle_start.elapsed();
        if elapsed < frame_interval {
            thread::sleep(frame_interval - elapsed);
        }
    }
}

fn open_device(cfg: &AppConfig) -> Result<std::fs::File> {
    configure_v4l2(cfg)?;
    OpenOptions::new()
        .write(true)
        .open(&cfg.loopback_device)
        .with_context(|| format!("failed to open output device: {}", cfg.loopback_device.display()))
}

fn configure_v4l2(cfg: &AppConfig) -> Result<()> {
    let dev = v4l::Device::with_path(&cfg.loopback_device)
        .with_context(|| format!("failed to open v4l2 device: {}", cfg.loopback_device.display()))?;

    let mut fmt = dev
        .format()
        .context("failed to read current v4l2 format")?;
    fmt.width = cfg.frame_width;
    fmt.height = cfg.frame_height;
    fmt.fourcc = FourCC::new(b"YUYV");

    let applied = dev
        .set_format(&fmt)
        .context("failed to set v4l2 output format")?;

    info!(
        width = applied.width,
        height = applied.height,
        fourcc = %applied.fourcc,
        "v4l2 format configured"
    );

    Ok(())
}
