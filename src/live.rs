use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, TrySendError};
use tracing::{info, warn};

use crate::config::AppConfig;

#[derive(Debug)]
pub enum LiveEvent {
    Frame(Vec<u8>),
    StreamUp,
    StreamDown,
}

pub fn spawn_live_reader(
    cfg: AppConfig,
    tx: Sender<LiveEvent>,
    recycle_rx: Receiver<Vec<u8>>,
) -> Result<thread::JoinHandle<()>> {
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

            let mut cmd = Command::new(&ffmpeg_bin);
            cmd.arg("-loglevel").arg("warning");
            cmd.arg("-fflags").arg("nobuffer");
            cmd.arg("-flags").arg("low_delay");
            cmd.arg("-probesize")
                .arg(cfg.ffmpeg_probesize_bytes.to_string());
            cmd.arg("-analyzeduration")
                .arg(cfg.ffmpeg_analyzeduration_us.to_string());
            cmd.arg("-max_delay").arg("0");
            
            if cfg.enable_hwaccel {
                cmd.arg("-hwaccel").arg("vaapi");
                cmd.arg("-hwaccel_output_format").arg("nv12");
            }

            let mut child = match cmd
                .arg("-i")
                .arg(&srt_url)
                .args(["-an", "-sn", "-dn"])
                .arg("-vf")
                .arg(format!(
                    "scale={}:{}:flags=fast_bilinear,fps={}",
                    cfg.frame_width, cfg.frame_height, cfg.fps
                ))
                .args(["-pix_fmt", "yuyv422", "-f", "rawvideo", "pipe:1"])
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
            let mut frame = recycle_rx
                .try_recv()
                .unwrap_or_else(|_| vec![0u8; frame_size]);

            loop {
                match stdout.read_exact(&mut frame) {
                    Ok(()) => {
                        if !had_live {
                            had_live = true;
                            let _ = tx.send(LiveEvent::StreamUp);
                        }
                        // Do not block on producer side; dropping old/new frames is better than latency buildup.
                        let send_frame = frame;
                        frame = recycle_rx
                            .try_recv()
                            .unwrap_or_else(|_| vec![0u8; frame_size]);

                        match tx.try_send(LiveEvent::Frame(send_frame)) {
                            Ok(()) => {
                            }
                            Err(TrySendError::Full(event)) => {
                                if let LiveEvent::Frame(unsent) = event {
                                    frame = unsent;
                                }
                            }
                            Err(TrySendError::Disconnected(_)) => {
                                let _ = child.kill();
                                let _ = child.wait();
                                return;
                            }
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
