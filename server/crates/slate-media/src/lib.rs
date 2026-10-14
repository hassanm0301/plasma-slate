//! Screen capture and low-latency encoding (ADR 0003).
//!
//! `pipewiresrc` (DMA-BUF from KWin) → `vapostproc` → `vah264enc`/`vah265enc` → `h264parse`
//! → `appsink`, with software (`x264enc`) and system-memory fallbacks.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Media errors.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("gstreamer init: {0}")]
    Init(String),
    #[error("no working encoder pipeline: {0}")]
    NoPipeline(String),
    #[error("pipeline error: {0}")]
    Pipeline(String),
}

/// Video codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    H264,
    Hevc,
}

/// Encoder configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct EncoderConfig {
    /// `pipewiresrc target-object` (PipeWire object serial or node id).
    pub pipewire_target: String,
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
    pub bitrate_kbps: u32,
    pub codec: Codec,
    /// Re-send the last captured frame after this much idle time so the stream never stalls.
    pub keepalive: Duration,
    /// Force software encoding (testing / no VA-API).
    pub software_only: bool,
}

impl EncoderConfig {
    pub fn new(pipewire_target: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            pipewire_target: pipewire_target.into(),
            width,
            height,
            max_fps: 60,
            bitrate_kbps: default_bitrate_kbps(width, height, 60),
            codec: Codec::H264,
            keepalive: Duration::from_millis(250),
            software_only: false,
        }
    }
}

/// Bitrate heuristic: ~0.14 bits per pixel per frame for desktop content, clamped 4–40 Mb/s.
pub fn default_bitrate_kbps(width: u32, height: u32, fps: u32) -> u32 {
    let bps = f64::from(width) * f64::from(height) * f64::from(fps.max(1)) * 0.14;
    ((bps / 1000.0) as u32).clamp(4_000, 40_000)
}

/// One encoded access unit (Annex-B).
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// Microseconds on [`now_us`]'s clock.
    pub capture_time_us: u64,
    pub encoded_time_us: u64,
}

/// Process-wide monotonic clock in microseconds (shared by all sessions for stats).
pub fn now_us() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    u64::try_from(EPOCH.get_or_init(Instant::now).elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// Initializes GStreamer once.
pub fn init() -> Result<(), MediaError> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| gst::init().map_err(|e| e.to_string()))
        .clone()
        .map_err(MediaError::Init)
}

/// Which hardware/software encoders exist on this machine.
pub fn available_encoders() -> Vec<&'static str> {
    if init().is_err() {
        return Vec::new();
    }
    [
        "vah264enc",
        "vah264lpenc",
        "vah265enc",
        "vah265lpenc",
        "x264enc",
    ]
    .into_iter()
    .filter(|n| gst::ElementFactory::find(n).is_some())
    .collect()
}

#[derive(Debug, Clone)]
struct Candidate {
    label: &'static str,
    desc: String,
}

fn candidates(cfg: &EncoderConfig) -> Vec<Candidate> {
    let src = format!(
        "pipewiresrc name=src target-object={} always-copy=false do-timestamp=true keepalive-time={} resend-last=true",
        cfg.pipewire_target,
        cfg.keepalive.as_millis()
    );
    let gop = cfg.max_fps.max(1) * 5;
    let kbps = cfg.bitrate_kbps;
    let (va, va_lp, parse, caps) = match cfg.codec {
        Codec::H264 => (
            "vah264enc",
            "vah264lpenc",
            "h264parse config-interval=-1",
            "video/x-h264,stream-format=byte-stream,alignment=au",
        ),
        Codec::Hevc => (
            "vah265enc",
            "vah265lpenc",
            "h265parse config-interval=-1",
            "video/x-h265,stream-format=byte-stream,alignment=au",
        ),
    };
    let sink = "appsink name=sink sync=false max-buffers=8 drop=false emit-signals=false";
    let va_opts = format!(
        "rate-control=cbr bitrate={kbps} key-int-max={gop} b-frames=0 ref-frames=1 target-usage=7"
    );
    let mut out = Vec::new();
    if !cfg.software_only {
        for enc in [va, va_lp] {
            if gst::ElementFactory::find(enc).is_none() {
                continue;
            }
            out.push(Candidate {
                label: "va-dmabuf",
                desc: format!(
                    "{src} ! video/x-raw(memory:DMABuf) ! vapostproc ! video/x-raw(memory:VAMemory),format=NV12 ! {enc} name=enc {va_opts} ! {parse} ! {caps} ! {sink}"
                ),
            });
            out.push(Candidate {
                label: "va-sysmem",
                desc: format!(
                    "{src} ! videoconvert ! vapostproc ! video/x-raw(memory:VAMemory),format=NV12 ! {enc} name=enc {va_opts} ! {parse} ! {caps} ! {sink}"
                ),
            });
        }
    }
    if cfg.codec == Codec::H264 && gst::ElementFactory::find("x264enc").is_some() {
        out.push(Candidate {
            label: "x264",
            desc: format!(
                "{src} ! videoconvert ! video/x-raw,format=I420 ! x264enc name=enc tune=zerolatency speed-preset=ultrafast \
                 bitrate={kbps} key-int-max={gop} bframes=0 sliced-threads=true ! {parse} ! {caps} ! {sink}"
            ),
        });
    }
    out
}

struct Shared {
    tx: mpsc::Sender<EncodedFrame>,
    /// Set when frames had to be dropped; non-keyframes are skipped until the next keyframe.
    resync: AtomicBool,
    frames: AtomicU64,
    bytes: AtomicU64,
    errors: Mutex<Option<mpsc::UnboundedSender<String>>>,
}

/// A running capture + encode pipeline. Dropping it stops the pipeline.
pub struct Encoder {
    pipeline: gst::Pipeline,
    sink: gst_app::AppSink,
    enc: gst::Element,
    shared: Arc<Shared>,
    label: &'static str,
    bus_thread: Option<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder")
            .field("pipeline", &self.label)
            .finish_non_exhaustive()
    }
}

/// Channels returned by [`Encoder::start`].
#[derive(Debug)]
pub struct EncoderOutputs {
    pub frames: mpsc::Receiver<EncodedFrame>,
    /// Fatal pipeline errors after startup.
    pub errors: mpsc::UnboundedReceiver<String>,
}

impl Encoder {
    /// Starts the first candidate pipeline that reaches PLAYING without an error within the
    /// probe window.
    pub fn start(cfg: &EncoderConfig) -> Result<(Encoder, EncoderOutputs), MediaError> {
        init()?;
        let mut last_err = String::from("no candidates (missing gst-plugin-va / x264?)");
        for cand in candidates(cfg) {
            debug!(label = cand.label, desc = %cand.desc, "trying encoder pipeline");
            match Self::try_start(&cand) {
                Ok(v) => {
                    info!(
                        label = cand.label,
                        width = cfg.width,
                        height = cfg.height,
                        kbps = cfg.bitrate_kbps,
                        "encoder running"
                    );
                    return Ok(v);
                }
                Err(e) => {
                    warn!(label = cand.label, "encoder pipeline failed: {e}");
                    last_err = e;
                }
            }
        }
        Err(MediaError::NoPipeline(last_err))
    }

    fn try_start(cand: &Candidate) -> Result<(Encoder, EncoderOutputs), String> {
        let pipeline = gst::parse::launch(&cand.desc)
            .map_err(|e| e.to_string())?
            .downcast::<gst::Pipeline>()
            .map_err(|_| "not a pipeline".to_string())?;
        let sink = pipeline
            .by_name("sink")
            .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
            .ok_or("no appsink")?;
        let enc = pipeline.by_name("enc").ok_or("no encoder element")?;

        let (tx, frames) = mpsc::channel(8);
        let (err_tx, errors) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            tx,
            resync: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            errors: Mutex::new(Some(err_tx)),
        });

        {
            let shared = shared.clone();
            let pipeline_weak = pipeline.downgrade();
            sink.set_callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |sink| {
                        let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        let Some(buffer) = sample.buffer() else {
                            return Ok(gst::FlowSuccess::Ok);
                        };
                        let keyframe = !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT);
                        if shared.resync.load(Ordering::Relaxed) {
                            if !keyframe {
                                return Ok(gst::FlowSuccess::Ok);
                            }
                            shared.resync.store(false, Ordering::Relaxed);
                        }
                        let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                        let encoded = now_us();
                        let latency_us = pipeline_weak
                            .upgrade()
                            .and_then(|p| {
                                let clock = p.clock()?;
                                let running = clock.time().checked_sub(p.base_time()?)?;
                                let pts = buffer.pts()?;
                                Some(running.saturating_sub(pts).useconds())
                            })
                            .unwrap_or(0);
                        let frame = EncodedFrame {
                            data: map.as_slice().to_vec(),
                            keyframe,
                            capture_time_us: encoded.saturating_sub(latency_us),
                            encoded_time_us: encoded,
                        };
                        shared.frames.fetch_add(1, Ordering::Relaxed);
                        shared
                            .bytes
                            .fetch_add(frame.data.len() as u64, Ordering::Relaxed);
                        match shared.tx.try_send(frame) {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                // Consumer is behind (slow network): drop and resync at the next keyframe.
                                shared.resync.store(true, Ordering::Relaxed);
                                request_keyframe_on(sink.upcast_ref());
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => {
                                return Err(gst::FlowError::Eos)
                            }
                        }
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .build(),
            );
        }

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("set PLAYING: {e}"))?;

        // Probe: wait for PLAYING or an error.
        let bus = pipeline.bus().ok_or("no bus")?;
        let deadline = Instant::now() + Duration::from_millis(2500);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let Some(msg) = bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(left.as_millis() as u64),
                &[
                    gst::MessageType::Error,
                    gst::MessageType::AsyncDone,
                    gst::MessageType::StateChanged,
                ],
            ) else {
                break;
            };
            match msg.view() {
                gst::MessageView::Error(e) => {
                    let _ = pipeline.set_state(gst::State::Null);
                    return Err(format!("{} ({:?})", e.error(), e.debug()));
                }
                gst::MessageView::StateChanged(s)
                    if msg
                        .src()
                        .map(|s| s == pipeline.upcast_ref::<gst::Object>())
                        .unwrap_or(false)
                        && s.current() == gst::State::Playing =>
                {
                    break;
                }
                _ => {}
            }
        }

        let stop = Arc::new(AtomicBool::new(false));
        let bus_thread = {
            let shared = shared.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("slate-gst-bus".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let Some(msg) = bus.timed_pop_filtered(
                            gst::ClockTime::from_mseconds(200),
                            &[
                                gst::MessageType::Error,
                                gst::MessageType::Eos,
                                gst::MessageType::Warning,
                            ],
                        ) else {
                            continue;
                        };
                        let text = match msg.view() {
                            gst::MessageView::Error(e) => {
                                format!("{} ({:?})", e.error(), e.debug())
                            }
                            gst::MessageView::Eos(_) => {
                                "end of stream (PipeWire stream closed)".to_string()
                            }
                            gst::MessageView::Warning(w) => {
                                warn!("gstreamer warning: {} ({:?})", w.error(), w.debug());
                                continue;
                            }
                            _ => continue,
                        };
                        if let Ok(g) = shared.errors.lock() {
                            if let Some(tx) = g.as_ref() {
                                let _ = tx.send(text);
                            }
                        }
                        break;
                    }
                })
                .map_err(|e| e.to_string())?
        };

        let encoder = Encoder {
            pipeline,
            sink,
            enc,
            shared,
            label: cand.label,
            bus_thread: Some(bus_thread),
            stop,
        };
        encoder.force_keyframe();
        Ok((encoder, EncoderOutputs { frames, errors }))
    }

    /// Which pipeline variant is running (`va-dmabuf`, `va-sysmem`, `x264`).
    pub fn variant(&self) -> &'static str {
        self.label
    }

    /// Asks the encoder for an IDR frame (with parameter sets) as soon as possible.
    pub fn force_keyframe(&self) {
        request_keyframe_on(self.sink.upcast_ref());
    }

    /// Changes the target bitrate while running.
    pub fn set_bitrate_kbps(&self, kbps: u32) {
        if self.enc.has_property("bitrate") {
            self.enc.set_property("bitrate", kbps);
            debug!(kbps, "encoder bitrate changed");
        }
    }

    /// (frames, bytes) produced so far.
    pub fn counters(&self) -> (u64, u64) {
        (
            self.shared.frames.load(Ordering::Relaxed),
            self.shared.bytes.load(Ordering::Relaxed),
        )
    }
}

fn request_keyframe_on(element: &gst::Element) {
    let ev = gstreamer_video::UpstreamForceKeyUnitEvent::builder()
        .all_headers(true)
        .build();
    if let Some(pad) = element.static_pad("sink") {
        let _ = pad.push_event(ev);
    } else {
        let _ = element.send_event(ev);
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut g) = self.shared.errors.lock() {
            g.take();
        }
        let _ = self.pipeline.set_state(gst::State::Null);
        if let Some(t) = self.bus_thread.take() {
            let _ = t.join();
        }
        debug!("encoder stopped");
    }
}

/// Splits an Annex-B byte stream into NAL unit types (H.264), for tests and diagnostics.
pub fn h264_nal_types(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < data.len() {
        let sc3 = data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1;
        let sc4 = i + 4 < data.len()
            && data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 0
            && data[i + 3] == 1;
        if sc3 || sc4 {
            let start = i + if sc4 { 4 } else { 3 };
            if start < data.len() {
                out.push(data[start] & 0x1f);
            }
            i = start;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_heuristic() {
        assert_eq!(default_bitrate_kbps(1920, 1200, 60), 19_353);
        assert_eq!(default_bitrate_kbps(640, 480, 30), 4_000);
        assert_eq!(default_bitrate_kbps(7680, 4320, 120), 40_000);
    }

    #[test]
    fn nal_types() {
        let d = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9,
        ];
        assert_eq!(h264_nal_types(&d), vec![7, 8, 5]);
    }

    /// Encodes a synthetic source through the same encoder branch to make sure the element
    /// properties are valid on this machine. Skipped when no encoder is installed.
    #[test]
    fn encoder_elements_accept_properties() {
        if init().is_err() {
            return;
        }
        for enc in available_encoders() {
            let props = if enc.starts_with("va") {
                "rate-control=cbr bitrate=8000 key-int-max=300 b-frames=0 ref-frames=1 target-usage=7"
            } else {
                "tune=zerolatency speed-preset=ultrafast bitrate=8000 key-int-max=300 bframes=0"
            };
            let pre = if enc.starts_with("va") {
                "vapostproc ! video/x-raw(memory:VAMemory),format=NV12 ! "
            } else {
                ""
            };
            let desc = format!(
                "videotestsrc num-buffers=10 ! video/x-raw,width=640,height=400,format=NV12 ! {pre}{enc} {props} ! fakesink"
            );
            let p = gst::parse::launch(&desc).unwrap_or_else(|e| panic!("{enc}: {e}"));
            p.set_state(gst::State::Playing).unwrap();
            let bus = p.bus().unwrap();
            let msg = bus.timed_pop_filtered(
                gst::ClockTime::from_seconds(10),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            );
            p.set_state(gst::State::Null).unwrap();
            match msg.map(|m| m.type_()) {
                Some(gst::MessageType::Eos) => {}
                other => panic!("{enc}: unexpected {other:?}"),
            }
        }
    }
}
