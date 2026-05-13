use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use tracing::{info, warn};
use v4l::FourCC;
use v4l::video::Output;

use crate::config::AppConfig;
use crate::live_event::LiveEvent;

pub fn run_output_loop(
    cfg: &AppConfig,
    dummy_frame: Vec<u8>,
    rx: Receiver<LiveEvent>,
    recycle_tx: Sender<Vec<u8>>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let mut out = open_device(cfg)?;
    let live_interval = cfg.frame_interval();
    let dummy_interval = Duration::from_millis(5000); // 0.2 fps for static dummy image
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

        // In dummy mode block on recv_timeout so we wake immediately on StreamUp
        // instead of spinning or sleeping through the full dummy interval.
        // In live mode drain non-blocking.
        if !live_active {
            match rx.recv_timeout(dummy_interval) {
                Ok(event) => process_event(event, &mut last_live_frame, &mut last_live_at, &mut live_active, &recycle_tx),
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
        while let Ok(event) = rx.try_recv() {
            process_event(event, &mut last_live_frame, &mut last_live_at, &mut live_active, &recycle_tx);
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
            match open_device(cfg) {
                Ok(new_out) => out = new_out,
                Err(err) => {
                    warn!(error = %err, "failed to reopen loopback device; will retry next frame");
                }
            }
            continue;
        }

        // In live mode pace to target fps; dummy mode already waited via recv_timeout.
        // Always sleep live_interval when live_active to avoid CPU spin during
        // the window between StreamUp and first Frame arrival, or after live_timeout.
        if live_active {
            let elapsed = cycle_start.elapsed();
            if elapsed < live_interval {
                thread::sleep(live_interval - elapsed);
            }
        }
    }
}

fn process_event(
    event: LiveEvent,
    last_live_frame: &mut Option<Vec<u8>>,
    last_live_at: &mut Option<Instant>,
    live_active: &mut bool,
    recycle_tx: &Sender<Vec<u8>>,
) {
    match event {
        LiveEvent::Frame(frame) => {
            if let Some(old) = last_live_frame.replace(frame) {
                let _ = recycle_tx.try_send(old);
            }
            *last_live_at = Some(Instant::now());
        }
        LiveEvent::StreamUp => {
            *live_active = true;
            *last_live_at = Some(Instant::now());
            info!("stream state: live");
        }
        LiveEvent::StreamDown => {
            *live_active = false;
            *last_live_at = None;
            info!("stream state: dummy (disconnected)");
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
