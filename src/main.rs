mod config;
mod dummy;
mod live;
#[cfg(feature = "ffmpeg-next-backend")]
mod live_ffmpeg_next;
mod output;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
#[cfg(not(feature = "ffmpeg-next-backend"))]
use anyhow::bail;
use clap::Parser;
use crossbeam_channel::bounded;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::config::{AppConfig, Cli, LiveBackend};
use crate::dummy::load_dummy_yuyv;
use crate::live::spawn_live_reader;
#[cfg(feature = "ffmpeg-next-backend")]
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
        hwaccel = cfg.enable_hwaccel,
        backend = ?cfg.live_backend,
        "rstcam starting"
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let flag = Arc::clone(&shutdown);
        ctrlc::set_handler(move || {
            flag.store(true, Ordering::SeqCst);
        })?;
    }

    let dummy_frame = load_dummy_yuyv(&cfg.dummy_image, cfg.frame_width, cfg.frame_height)?;

    let (tx, rx) = bounded(cfg.live_channel_capacity);
    let (recycle_tx, recycle_rx) = bounded::<Vec<u8>>(2);
    let _live_handle = match cfg.live_backend {
        LiveBackend::Subprocess => spawn_live_reader(cfg.clone(), tx, recycle_rx)?,
        LiveBackend::FfmpegNext => {
            #[cfg(feature = "ffmpeg-next-backend")]
            {
                spawn_live_reader_ffmpeg_next(cfg.clone(), tx, recycle_rx)?
            }
            #[cfg(not(feature = "ffmpeg-next-backend"))]
            {
                bail!(
                    "live_backend=ffmpeg-next requires build feature 'ffmpeg-next-backend'"
                )
            }
        }
    };

    run_output_loop(&cfg, dummy_frame, rx, recycle_tx, shutdown)
}
