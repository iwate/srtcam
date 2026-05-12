mod config;
mod dummy;
mod live;
mod output;

use anyhow::Result;
use clap::Parser;
use crossbeam_channel::bounded;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::config::{AppConfig, Cli};
use crate::dummy::load_dummy_yuyv;
use crate::live::spawn_live_reader;
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
        device = %cfg.loopback_device.display(),
        width = cfg.width,
        height = cfg.height,
        fps = cfg.fps,
        "rstcam starting"
    );

    let dummy_frame = load_dummy_yuyv(&cfg.dummy_image, cfg.width, cfg.height)?;

    let (tx, rx) = bounded(4);
    let _live_handle = spawn_live_reader(cfg.clone(), tx)?;

    run_output_loop(&cfg, dummy_frame, rx)
}
