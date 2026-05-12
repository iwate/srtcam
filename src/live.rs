use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use tracing::{info, warn};

use crate::config::AppConfig;

#[derive(Debug)]
pub enum LiveEvent {
    Frame(Vec<u8>),
    StreamUp,
    StreamDown,
}

pub fn spawn_live_reader(cfg: AppConfig, tx: Sender<LiveEvent>) -> Result<thread::JoinHandle<()>> {
    let ffmpeg_bin = which::which(&cfg.ffmpeg_bin)
        .with_context(|| format!("failed to locate ffmpeg binary: {}", cfg.ffmpeg_bin))?;
    let ffmpeg_bin = ffmpeg_bin.to_string_lossy().to_string();

    let handle = thread::spawn(move || {
        let frame_size = cfg.frame_size_bytes();

        loop {
            let srt_url = format!(
                "srt://0.0.0.0:{}?mode=listener&latency={}",
                cfg.listen_port, cfg.srt_latency_ms
            );

            let mut child = match Command::new(&ffmpeg_bin)
                .args([
                    "-loglevel",
                    "warning",
                    "-fflags",
                    "nobuffer",
                    "-flags",
                    "low_delay",
                    "-i",
                    &srt_url,
                    "-an",
                    "-sn",
                    "-dn",
                    "-vf",
                    &format!("scale={}:{},fps={}", cfg.width, cfg.height, cfg.fps),
                    "-pix_fmt",
                    "yuyv422",
                    "-f",
                    "rawvideo",
                    "pipe:1",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
            {
                Ok(c) => c,
                Err(err) => {
                    warn!(error = %err, "failed to start ffmpeg; retrying");
                    thread::sleep(Duration::from_millis(1000));
                    continue;
                }
            };

            let mut stdout = match child.stdout.take() {
                Some(s) => s,
                None => {
                    warn!("ffmpeg stdout unavailable");
                    let _ = child.kill();
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }
            };

            info!("waiting for SRT stream on configured port");
            let mut had_live = false;
            let mut frame = vec![0u8; frame_size];

            loop {
                match stdout.read_exact(&mut frame) {
                    Ok(()) => {
                        if !had_live {
                            had_live = true;
                            let _ = tx.send(LiveEvent::StreamUp);
                        }
                        // Best effort: if channel is full, drop the oldest by retrying once.
                        if tx.try_send(LiveEvent::Frame(frame.clone())).is_err() {
                            let _ = tx.send(LiveEvent::Frame(frame.clone()));
                        }
                    }
                    Err(err) => {
                        if had_live {
                            let _ = tx.send(LiveEvent::StreamDown);
                        }
                        warn!(error = %err, "live stream ended or decode stalled; restarting listener");
                        break;
                    }
                }
            }

            let _ = child.kill();
            let _ = child.wait();
            thread::sleep(Duration::from_millis(300));
        }
    });

    Ok(handle)
}
