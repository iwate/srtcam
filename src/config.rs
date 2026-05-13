use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub listen_port: u16,
    pub srt_latency_ms: u32,
    pub loopback_device: PathBuf,
    pub frame_width: u32,
    pub frame_height: u32,
    pub fps: u32,
    pub live_timeout_ms: u64,
    pub live_channel_capacity: usize,
    pub latency_profile: LatencyProfile,
    pub ffmpeg_analyzeduration_us: u64,
    pub ffmpeg_probesize_bytes: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LatencyProfile {
    Balanced,
    UltraLow,
}

#[derive(Debug, Parser)]
#[command(name = "rstcam", about = "SRT to v4l2loopback bridge")]
pub struct Cli {
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long)]
    pub listen_port: Option<u16>,
    #[arg(long)]
    pub srt_latency_ms: Option<u32>,
    #[arg(long)]
    pub loopback_device: Option<PathBuf>,
    #[arg(long)]
    pub frame_width: Option<u32>,
    #[arg(long)]
    pub frame_height: Option<u32>,
    #[arg(long)]
    pub width: Option<u32>,
    #[arg(long)]
    pub height: Option<u32>,
    #[arg(long)]
    pub fps: Option<u32>,
    #[arg(long)]
    pub live_timeout_ms: Option<u64>,
    #[arg(long)]
    pub live_channel_capacity: Option<usize>,
    #[arg(long, value_enum)]
    pub latency_profile: Option<LatencyProfile>,
    #[arg(long)]
    pub ffmpeg_analyzeduration_us: Option<u64>,
    #[arg(long)]
    pub ffmpeg_probesize_bytes: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    listen_port: Option<u16>,
    srt_latency_ms: Option<u32>,
    loopback_device: Option<PathBuf>,
    frame_width: Option<u32>,
    frame_height: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<u32>,
    live_timeout_ms: Option<u64>,
    live_channel_capacity: Option<usize>,
    latency_profile: Option<LatencyProfile>,
    ffmpeg_analyzeduration_us: Option<u64>,
    ffmpeg_probesize_bytes: Option<u64>,
}

impl AppConfig {
    pub fn load(cli: Cli) -> Result<Self> {
        let file_cfg = if let Some(path) = &cli.config {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("failed to read config file: {}", path.display()))?;
            toml::from_str::<FileConfig>(&raw)
                .with_context(|| format!("failed to parse TOML config: {}", path.display()))?
        } else {
            FileConfig::default()
        };

        let latency_profile = cli
            .latency_profile
            .or(file_cfg.latency_profile)
            .unwrap_or(LatencyProfile::Balanced);

        let default_srt_latency_ms = match latency_profile {
            LatencyProfile::Balanced => 120,
            LatencyProfile::UltraLow => 30,
        };
        let default_live_timeout_ms = match latency_profile {
            LatencyProfile::Balanced => 800,
            LatencyProfile::UltraLow => 300,
        };
        let default_live_channel_capacity = match latency_profile {
            LatencyProfile::Balanced => 4,
            LatencyProfile::UltraLow => 1,
        };

        let cfg = Self {
            listen_port: cli
                .listen_port
                .or(file_cfg.listen_port)
                .unwrap_or(5000),
            srt_latency_ms: cli
                .srt_latency_ms
                .or(file_cfg.srt_latency_ms)
                .unwrap_or(default_srt_latency_ms),
            loopback_device: cli
                .loopback_device
                .or(file_cfg.loopback_device)
                .unwrap_or_else(|| PathBuf::from("/dev/video10")),
            frame_width: cli
                .frame_width
                .or(cli.width)
                .or(file_cfg.frame_width)
                .or(file_cfg.width)
                .unwrap_or(1280),
            frame_height: cli
                .frame_height
                .or(cli.height)
                .or(file_cfg.frame_height)
                .or(file_cfg.height)
                .unwrap_or(720),
            fps: cli.fps.or(file_cfg.fps).unwrap_or(30),
            live_timeout_ms: cli
                .live_timeout_ms
                .or(file_cfg.live_timeout_ms)
                .unwrap_or(default_live_timeout_ms),
            live_channel_capacity: cli
                .live_channel_capacity
                .or(file_cfg.live_channel_capacity)
                .unwrap_or(default_live_channel_capacity),
            latency_profile,
            ffmpeg_analyzeduration_us: cli
                .ffmpeg_analyzeduration_us
                .or(file_cfg.ffmpeg_analyzeduration_us)
                .unwrap_or(0),
            ffmpeg_probesize_bytes: cli
                .ffmpeg_probesize_bytes
                .or(file_cfg.ffmpeg_probesize_bytes)
                .unwrap_or(500_000),
        };

        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        if self.listen_port == 0 {
            anyhow::bail!("listen_port must be > 0");
        }
        if self.fps == 0 {
            anyhow::bail!("fps must be > 0");
        }
        if self.frame_width == 0 || self.frame_height == 0 {
            anyhow::bail!("frame_width/frame_height must be > 0");
        }
        if self.srt_latency_ms == 0 {
            anyhow::bail!("srt_latency_ms must be > 0");
        }
        if self.live_channel_capacity == 0 {
            anyhow::bail!("live_channel_capacity must be > 0");
        }
        if self.ffmpeg_probesize_bytes == 0 {
            anyhow::bail!("ffmpeg_probesize_bytes must be > 0");
        }
        if !self.loopback_device.exists() {
            anyhow::bail!(
                "loopback device does not exist: {}",
                self.loopback_device.display()
            );
        }
        Ok(())
    }

    pub fn frame_size_bytes(&self) -> usize {
        // yuyv422 => 2 bytes per pixel
        (self.frame_width as usize) * (self.frame_height as usize) * 2
    }

    pub fn frame_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(1.0 / self.fps as f64)
    }
}
