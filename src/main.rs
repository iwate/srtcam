mod backoff;
mod config;
mod dummy;
mod live_ffmpeg_next;
mod live_event;
mod output;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use clap::Parser;
use crossbeam_channel::bounded;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::config::{AppConfig, Cli};
use crate::dummy::black_dummy_yuyv;
use crate::live_ffmpeg_next::spawn_live_reader_ffmpeg_next;
use crate::output::run_output_loop;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let cfg = AppConfig::load(cli)?;

    info!(
        port = cfg.listen_port,
        latency_ms = cfg.srt_latency_ms,
        profile = ?cfg.latency_profile,
        queue = cfg.live_channel_capacity,
        device = %cfg.loopback_device.display(),
        frame_width = cfg.frame_width,
        frame_height = cfg.frame_height,
        fps = cfg.fps,
        "rstcam starting"
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let flag = Arc::clone(&shutdown);
        ctrlc::set_handler(move || {
            flag.store(true, Ordering::SeqCst);
        })?;
    }

    let dummy_frame = black_dummy_yuyv(cfg.frame_width, cfg.frame_height);

    let (tx, rx) = bounded(cfg.live_channel_capacity);
    let (recycle_tx, recycle_rx) = bounded::<Vec<u8>>(2);
    let _live_handle =
        spawn_live_reader_ffmpeg_next(cfg.clone(), tx, recycle_rx, Arc::clone(&shutdown))?;

    run_output_loop(&cfg, dummy_frame, rx, recycle_tx, shutdown)
}
