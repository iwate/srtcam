use std::ffi::CString;
use std::os::raw::{c_int, c_void};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, TrySendError};
use ffmpeg_next as ffmpeg;
use tracing::{info, warn};

use crate::backoff::{ExponentialBackoff, wait_or_shutdown};
use crate::config::AppConfig;
use crate::live_event::LiveEvent;

const INTERRUPT_REASON_NONE: u8 = 0;
const INTERRUPT_REASON_IDLE: u8 = 1;
const INTERRUPT_REASON_SHUTDOWN: u8 = 2;

struct InterruptState {
    shutdown: Arc<AtomicBool>,
    opened_at: Instant,
    last_progress_ms: Arc<AtomicU64>,
    idle_timeout_ms: u64,
    reason: Arc<AtomicU8>,
}

impl InterruptState {
    fn should_interrupt(&self) -> bool {
        if self.shutdown.load(Ordering::SeqCst) {
            self.reason
                .store(INTERRUPT_REASON_SHUTDOWN, Ordering::SeqCst);
            return true;
        }

        let elapsed_ms = self.opened_at.elapsed().as_millis() as u64;
        let last_progress_ms = self.last_progress_ms.load(Ordering::SeqCst);
        if elapsed_ms.saturating_sub(last_progress_ms) > self.idle_timeout_ms {
            self.reason.store(INTERRUPT_REASON_IDLE, Ordering::SeqCst);
            return true;
        }

        false
    }
}

extern "C" fn ffmpeg_interrupt_callback(opaque: *mut c_void) -> c_int {
    let state = unsafe { &mut *(opaque as *mut InterruptState) };
    state.should_interrupt() as c_int
}

struct InputContextWithInterrupt {
    input: Option<ffmpeg::format::context::Input>,
    opaque: *mut InterruptState,
    opened_at: Instant,
    last_progress_ms: Arc<AtomicU64>,
    reason: Arc<AtomicU8>,
}

impl InputContextWithInterrupt {
    fn input_mut(&mut self) -> &mut ffmpeg::format::context::Input {
        self.input.as_mut().expect("input context must exist")
    }

    fn mark_progress(&self) {
        let elapsed_ms = self.opened_at.elapsed().as_millis() as u64;
        self.last_progress_ms.store(elapsed_ms, Ordering::SeqCst);
        self.reason.store(INTERRUPT_REASON_NONE, Ordering::SeqCst);
    }

    fn take_reason(&self) -> u8 {
        self.reason.swap(INTERRUPT_REASON_NONE, Ordering::SeqCst)
    }
}

impl Drop for InputContextWithInterrupt {
    fn drop(&mut self) {
        if let Some(input) = self.input.take() {
            drop(input);
        }

        unsafe {
            drop(Box::from_raw(self.opaque));
        }
    }
}

fn open_input_with_dictionary_interrupt(
    srt_url: &str,
    options: ffmpeg::Dictionary,
    shutdown: Arc<AtomicBool>,
    idle_timeout: Duration,
) -> std::result::Result<InputContextWithInterrupt, ffmpeg::Error> {
    unsafe {
        let mut format_context = ffmpeg::ffi::avformat_alloc_context();
        let opened_at = Instant::now();
        let last_progress_ms = Arc::new(AtomicU64::new(0));
        let reason = Arc::new(AtomicU8::new(INTERRUPT_REASON_NONE));
        let interrupt_state = Box::new(InterruptState {
            shutdown,
            opened_at,
            last_progress_ms: Arc::clone(&last_progress_ms),
            idle_timeout_ms: idle_timeout.as_millis() as u64,
            reason: Arc::clone(&reason),
        });
        let opaque = Box::into_raw(interrupt_state);

        (*format_context).interrupt_callback = ffmpeg::ffi::AVIOInterruptCB {
            callback: Some(ffmpeg_interrupt_callback),
            opaque: opaque as *mut c_void,
        };

        let path = CString::new(srt_url).map_err(|_| ffmpeg::Error::InvalidData)?;
        let mut opts = options.disown();
        let open_result =
            ffmpeg::ffi::avformat_open_input(&mut format_context, path.as_ptr(), ptr::null_mut(), &mut opts);
        ffmpeg::Dictionary::own(opts);

        if open_result != 0 {
            if !format_context.is_null() {
                ffmpeg::ffi::avformat_close_input(&mut format_context);
            }
            drop(Box::from_raw(opaque));
            return Err(ffmpeg::Error::from(open_result));
        }

        let stream_info_result = ffmpeg::ffi::avformat_find_stream_info(format_context, ptr::null_mut());
        if stream_info_result < 0 {
            ffmpeg::ffi::avformat_close_input(&mut format_context);
            drop(Box::from_raw(opaque));
            return Err(ffmpeg::Error::from(stream_info_result));
        }

        let interrupt_input = InputContextWithInterrupt {
            input: Some(ffmpeg::format::context::Input::wrap(format_context)),
            opaque,
            opened_at,
            last_progress_ms,
            reason,
        };
        interrupt_input.mark_progress();
        Ok(interrupt_input)
    }
}

pub fn spawn_live_reader_ffmpeg_next(
    cfg: AppConfig,
    tx: Sender<LiveEvent>,
    recycle_rx: Receiver<Vec<u8>>,
    shutdown: Arc<AtomicBool>,
) -> Result<thread::JoinHandle<()>> {
    ffmpeg::init().context("failed to initialize ffmpeg-next")?;

    let handle = thread::spawn(move || {
        let frame_size = cfg.frame_size_bytes();
        let mut reconnect_backoff =
            ExponentialBackoff::new(Duration::from_millis(300), Duration::from_secs(30), 1.5);

        loop {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }

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
            let io_timeout_us = Duration::from_millis(cfg.live_timeout_ms.saturating_mul(3).max(1000))
                .as_micros()
                .to_string();
            dict.set("rw_timeout", &io_timeout_us);

            let idle_timeout = Duration::from_millis(cfg.live_timeout_ms.saturating_mul(3).max(1000));

            let mut input_context = match open_input_with_dictionary_interrupt(
                &srt_url,
                dict,
                Arc::clone(&shutdown),
                idle_timeout,
            ) {
                Ok(v) => v,
                Err(err) => {
                    let delay = reconnect_backoff.next_delay();
                    warn!(
                        error = %err,
                        delay_ms = delay.as_millis(),
                        "ffmpeg-next failed to open input; retrying with backoff"
                    );
                    if !wait_or_shutdown(delay, &shutdown) {
                        break;
                    }
                    continue;
                }
            };

            // Scope `input` inside a block so its borrow on `ictx` ends here.
            // This lets us explicitly `drop(ictx)` after the stream ends, which
            // closes the SRT socket and releases the port before we sleep.
            let (video_stream_index, context_decoder) = {
                let input = match input_context
                    .input_mut()
                    .streams()
                    .best(ffmpeg::media::Type::Video)
                {
                    Some(v) => v,
                    None => {
                        let delay = reconnect_backoff.next_delay();
                        warn!(
                            delay_ms = delay.as_millis(),
                            "ffmpeg-next could not find video stream; retrying with backoff"
                        );
                        drop(input_context);
                        if !wait_or_shutdown(delay, &shutdown) {
                            break;
                        }
                        continue;
                    }
                };
                let idx = input.index();
                let ctx = match ffmpeg::codec::context::Context::from_parameters(input.parameters()) {
                    Ok(v) => v,
                    Err(err) => {
                        let delay = reconnect_backoff.next_delay();
                        warn!(
                            error = %err,
                            delay_ms = delay.as_millis(),
                            "ffmpeg-next failed to build codec context; retrying with backoff"
                        );
                        drop(input_context);
                        if !wait_or_shutdown(delay, &shutdown) {
                            break;
                        }
                        continue;
                    }
                };
                (idx, ctx) // `input` dropped here, borrow on `ictx` released
            };

            let mut decoder = match context_decoder.decoder().video() {
                Ok(v) => v,
                Err(err) => {
                    let delay = reconnect_backoff.next_delay();
                    warn!(
                        error = %err,
                        delay_ms = delay.as_millis(),
                        "ffmpeg-next failed to create video decoder; retrying with backoff"
                    );
                    drop(input_context);
                    if !wait_or_shutdown(delay, &shutdown) {
                        break;
                    }
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
                    let delay = reconnect_backoff.next_delay();
                    warn!(
                        error = %err,
                        delay_ms = delay.as_millis(),
                        "ffmpeg-next failed to create scaler; retrying with backoff"
                    );
                    drop(input_context);
                    if !wait_or_shutdown(delay, &shutdown) {
                        break;
                    }
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
                                    scaler: &mut ffmpeg::software::scaling::Context,
                                    input_context: &InputContextWithInterrupt|
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
                    input_context.mark_progress();

                    match tx.try_send(LiveEvent::Frame(frame)) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {}
                        Err(TrySendError::Disconnected(_)) => return false,
                    }
                }
                true
            };

            let end_reason = loop {
                let mut packet = ffmpeg::Packet::empty();
                match packet.read(input_context.input_mut()) {
                    Ok(()) => {
                        input_context.mark_progress();
                    }
                    Err(ffmpeg::Error::Eof) => {
                        break "eof";
                    }
                    Err(ffmpeg::Error::Exit) => {
                        break match input_context.take_reason() {
                            INTERRUPT_REASON_IDLE => "idle-timeout",
                            INTERRUPT_REASON_SHUTDOWN => "shutdown",
                            _ => "interrupt",
                        };
                    }
                    Err(ffmpeg::Error::Other { errno })
                        if errno == ffmpeg::error::ETIMEDOUT || errno == ffmpeg::error::EAGAIN =>
                    {
                        break "io-timeout";
                    }
                    Err(err) => {
                        warn!(error = %err, "ffmpeg-next packet read failed; restarting listener");
                        break "packet-read-error";
                    }
                }

                if packet.stream() != video_stream_index {
                    continue;
                }

                if decoder.send_packet(&packet).is_err() {
                    break "decoder-send-packet";
                }

                let was_live = had_live;
                if !send_decoded(
                    &mut decoder,
                    &tx,
                    &recycle_rx,
                    &mut had_live,
                    &mut src_frame,
                    &mut dst_frame,
                    &mut scaler,
                    &input_context,
                ) {
                    break "decode-or-channel";
                }

                if !was_live && had_live {
                    reconnect_backoff.reset();
                }
            };

            let _ = decoder.send_eof();
            if !send_decoded(
                &mut decoder,
                &tx,
                &recycle_rx,
                &mut had_live,
                &mut src_frame,
                &mut dst_frame,
                &mut scaler,
                &input_context,
            ) {
                warn!(had_live, "ffmpeg-next decoder flush failed");
            }

            // Drop ictx HERE — before sleeping — to close the SRT socket and
            // release the port immediately. If we sleep first, the port stays
            // bound and the next input_with_dictionary call fails with EADDRINUSE.
            drop(input_context);

            // Always send StreamDown so the output loop resets live_active,
            // even if this session ended before producing any decoded frames.
            let _ = tx.send(LiveEvent::StreamDown);

            warn!(reason = end_reason, had_live, "ffmpeg-next stream ended; restarting listener");

            if shutdown.load(Ordering::SeqCst) {
                break;
            }

            let delay = reconnect_backoff.next_delay();
            if !wait_or_shutdown(delay, &shutdown) {
                break;
            }
        }
    });

    Ok(handle)
}
