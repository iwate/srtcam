use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, TrySendError};
use ffmpeg_next as ffmpeg;
use tracing::{info, warn};

use crate::config::AppConfig;
use crate::live::LiveEvent;

pub fn spawn_live_reader_ffmpeg_next(
    cfg: AppConfig,
    tx: Sender<LiveEvent>,
    recycle_rx: Receiver<Vec<u8>>,
) -> Result<thread::JoinHandle<()>> {
    ffmpeg::init().context("failed to initialize ffmpeg-next")?;

    let handle = thread::spawn(move || {
        let frame_size = cfg.frame_size_bytes();

        loop {
            let srt_url = format!(
                "srt://0.0.0.0:{}?mode=listener&latency={}",
                cfg.listen_port, cfg.srt_latency_ms
            );

            let mut dict = ffmpeg::Dictionary::new();
            dict.set("fflags", "nobuffer");
            dict.set("flags", "low_delay");
            dict.set("probesize", &cfg.ffmpeg_probesize_bytes.to_string());
            dict.set("analyzeduration", &cfg.ffmpeg_analyzeduration_us.to_string());
            dict.set("max_delay", "0");

            let mut ictx = match ffmpeg::format::input_with_dictionary(&srt_url, dict) {
                Ok(v) => v,
                Err(err) => {
                    warn!(error = %err, "ffmpeg-next failed to open input; retrying");
                    thread::sleep(Duration::from_millis(1000));
                    continue;
                }
            };

            let input = match ictx.streams().best(ffmpeg::media::Type::Video) {
                Some(v) => v,
                None => {
                    warn!("ffmpeg-next could not find video stream; retrying");
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }
            };
            let video_stream_index = input.index();

            let context_decoder = match ffmpeg::codec::context::Context::from_parameters(input.parameters()) {
                Ok(v) => v,
                Err(err) => {
                    warn!(error = %err, "ffmpeg-next failed to build codec context; retrying");
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }
            };

            let mut decoder = match context_decoder.decoder().video() {
                Ok(v) => v,
                Err(err) => {
                    warn!(error = %err, "ffmpeg-next failed to create video decoder; retrying");
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }
            };

            let mut scaler = match ffmpeg::software::scaling::Context::get(
                decoder.format(),
                decoder.width(),
                decoder.height(),
                ffmpeg::format::Pixel::YUYV422,
                cfg.frame_width,
                cfg.frame_height,
                ffmpeg::software::scaling::flag::Flags::FAST_BILINEAR,
            ) {
                Ok(v) => v,
                Err(err) => {
                    warn!(error = %err, "ffmpeg-next failed to create scaler; retrying");
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }
            };

            info!("waiting for SRT stream on configured port (ffmpeg-next backend)");
            let mut had_live = false;
            let mut src_frame = ffmpeg::util::frame::video::Video::empty();
            let mut dst_frame = ffmpeg::util::frame::video::Video::empty();

            let send_decoded = |decoder: &mut ffmpeg::decoder::Video,
                                    tx: &Sender<LiveEvent>,
                                    recycle_rx: &Receiver<Vec<u8>>,
                                    had_live: &mut bool,
                                    src_frame: &mut ffmpeg::util::frame::video::Video,
                                    dst_frame: &mut ffmpeg::util::frame::video::Video,
                                    scaler: &mut ffmpeg::software::scaling::Context|
             -> bool {
                while decoder.receive_frame(src_frame).is_ok() {
                    if scaler.run(src_frame, dst_frame).is_err() {
                        return false;
                    }

                    let linesize = dst_frame.stride(0);
                    let data = dst_frame.data(0);
                    let row_bytes = (cfg.frame_width as usize) * 2;
                    let mut frame = recycle_rx
                        .try_recv()
                        .unwrap_or_else(|_| vec![0u8; frame_size]);

                    if frame.len() != frame_size {
                        frame.resize(frame_size, 0);
                    }

                    for y in 0..(cfg.frame_height as usize) {
                        let src_start = y * linesize;
                        let src_end = src_start + row_bytes;
                        let dst_start = y * row_bytes;
                        let dst_end = dst_start + row_bytes;
                        if src_end > data.len() || dst_end > frame.len() {
                            return false;
                        }
                        frame[dst_start..dst_end].copy_from_slice(&data[src_start..src_end]);
                    }

                    if !*had_live {
                        *had_live = true;
                        let _ = tx.send(LiveEvent::StreamUp);
                    }

                    match tx.try_send(LiveEvent::Frame(frame)) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {}
                        Err(TrySendError::Disconnected(_)) => return false,
                    }
                }
                true
            };

            let mut stream_ok = true;
            'read_loop: for (stream, packet) in ictx.packets() {
                if stream.index() != video_stream_index {
                    continue;
                }

                if decoder.send_packet(&packet).is_err() {
                    stream_ok = false;
                    break 'read_loop;
                }

                if !send_decoded(
                    &mut decoder,
                    &tx,
                    &recycle_rx,
                    &mut had_live,
                    &mut src_frame,
                    &mut dst_frame,
                    &mut scaler,
                ) {
                    stream_ok = false;
                    break 'read_loop;
                }
            }

            let _ = decoder.send_eof();
            let _ = send_decoded(
                &mut decoder,
                &tx,
                &recycle_rx,
                &mut had_live,
                &mut src_frame,
                &mut dst_frame,
                &mut scaler,
            );

            if had_live {
                let _ = tx.send(LiveEvent::StreamDown);
            }

            if !stream_ok {
                warn!("ffmpeg-next stream ended or decode stalled; restarting listener");
            }
            thread::sleep(Duration::from_millis(300));
        }
    });

    Ok(handle)
}
