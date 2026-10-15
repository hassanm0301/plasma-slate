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

/// Rate control of the hardware encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RateControl {
    /// Every frame is padded to `bitrate / fps` (Mesa pads even repeats of a static screen), so
    /// the stream is a constant, predictable `bitrate` on the wire.
    #[default]
    Cbr,
    /// Frames are as small as the content allows (a static screen costs a few dozen bytes per
    /// frame); `bitrate` is the ceiling. The way to go with filler frames.
    Vbr,
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
    /// With `filler_frames` > 0 this is the spacing of the repeats between real frames.
    pub keepalive: Duration,
    /// Force software encoding (testing / no VA-API).
    pub software_only: bool,
    /// VA `target-usage` (1–7). The scale is driver-specific: Intel runs fastest at 7, Mesa
    /// (AMD) at 1–3 (see [`default_target_usage`]).
    pub target_usage: u32,
    /// Keyframe interval in frames; 0 = as rare as the encoder allows (TCP never loses frames,
    /// and the tablet asks for a keyframe when it needs one), so no periodic size spikes.
    pub gop_frames: u32,
    /// Coded picture buffer size in frames (0 = encoder default). 1–2 caps every frame near
    /// `bitrate / fps`: no multi-frame bursts after a keyframe or a big change, at the price of
    /// a blurrier first frame after one.
    pub cpb_frames: u32,
    /// Insert access-unit delimiters.
    pub aud: bool,
    /// Repeats of the last picture sent between real frames (0 = off). Hardware decoders that
    /// hold a couple of frames release them sooner when more input arrives; a repeat costs a
    /// full encode on the PC but only a few dozen bytes on the wire (with [`RateControl::Vbr`]).
    pub filler_frames: u32,
    pub rate_control: RateControl,
    /// Slices per frame (1 = one). Software decoders can decode slices on several cores.
    pub num_slices: u32,
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
            target_usage: default_target_usage(),
            gop_frames: 0,
            cpb_frames: 0,
            aud: false,
            filler_frames: 0,
            rate_control: RateControl::Cbr,
            num_slices: 1,
        }
    }

    /// Spacing between the frames the encoder sees: real frames at `max_fps` plus
    /// `filler_frames` repeats in between (at least 2 ms apart).
    pub fn frame_spacing(&self) -> Duration {
        let per_second = u64::from(self.max_fps.max(1)) * u64::from(self.filler_frames + 1);
        Duration::from_millis((1000 / per_second).max(2))
    }
}

/// PCI vendor of the render node GStreamer's `va*` elements use (`/dev/dri/renderD128`).
fn render_node_vendor() -> Option<u32> {
    let s = std::fs::read_to_string("/sys/class/drm/renderD128/device/vendor").ok()?;
    u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
}

/// Fastest `target-usage` for this GPU. The VA-API scale is 1 = best quality … 7 = fastest,
/// which Intel's driver follows; Mesa's AMD encoder runs its presets the other way round
/// (measured on an RX 9070 XT at 1920×1200 HEVC: 1–3 ≈ 300 fps, 4–5 ≈ 150 fps, 6–7 ≈ 60 fps).
pub fn default_target_usage() -> u32 {
    const AMD: u32 = 0x1002;
    match render_node_vendor() {
        Some(AMD) => 1,
        _ => 7,
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
    /// A repeat of the previous picture (see [`EncoderConfig::filler_frames`]).
    pub filler: bool,
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
    max_fps: u32,
}

/// Frame-rate limiter applied before the encoder: lets frames through on an average of
/// `fps` per second (token schedule with a little slack so a 60 Hz source divides cleanly into
/// 30 or 20, and odd targets like 24 average out correctly).
#[derive(Debug)]
pub struct FrameLimiter {
    interval: Duration,
    next_due: Option<Instant>,
}

impl FrameLimiter {
    pub fn new(fps: u32) -> Self {
        Self {
            interval: Duration::from_secs_f64(1.0 / f64::from(fps.max(1))),
            next_due: None,
        }
    }

    /// Whether a frame arriving at `now` should be kept.
    pub fn admit(&mut self, now: Instant) -> bool {
        let slack = Duration::from_millis(3);
        match self.next_due {
            Some(due) if now + slack < due => false,
            Some(due) => {
                // Stay on schedule, but don't build up credit after an idle gap.
                let next = due + self.interval;
                self.next_due = Some(if next + self.interval < now {
                    now + self.interval
                } else {
                    next
                });
                true
            }
            None => {
                self.next_due = Some(now + self.interval);
                true
            }
        }
    }
}

fn candidates(cfg: &EncoderConfig) -> Vec<Candidate> {
    let src = format!(
        "pipewiresrc name=src target-object={} always-copy=false do-timestamp=true keepalive-time={} resend-last=true",
        cfg.pipewire_target,
        cfg.keepalive.as_millis(),
    );
    let kbps = cfg.bitrate_kbps;
    let fps = cfg.max_fps.max(1);
    // VA encoders accept at most 1024 frames between keyframes; x264 has no practical limit.
    let (va_gop, x264_gop) = if cfg.gop_frames == 0 {
        (1024, 1 << 30)
    } else {
        (cfg.gop_frames.min(1024), cfg.gop_frames)
    };
    let tu = cfg.target_usage.clamp(1, 7);
    let aud = cfg.aud;
    let (va_cpb, x264_cpb) = if cfg.cpb_frames > 0 {
        (
            format!(" cpb-size={}", cpb_size_kb(kbps, cfg.cpb_frames, fps)),
            format!(" vbv-buf-capacity={}", cfg.cpb_frames * 1000 / fps),
        )
    } else {
        (String::new(), String::new())
    };
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
    let rc = match cfg.rate_control {
        RateControl::Cbr => "rate-control=cbr",
        // `bitrate` is the maximum in VBR; aim a little below it.
        RateControl::Vbr => "rate-control=vbr target-percentage=70",
    };
    let slices = cfg.num_slices.clamp(1, 32);
    let va_opts = format!(
        "{rc} bitrate={kbps} key-int-max={va_gop} b-frames=0 ref-frames=1 target-usage={tu} aud={aud} num-slices={slices}{va_cpb}"
    );
    let mut out = Vec::new();
    if !cfg.software_only {
        for enc in [va, va_lp] {
            if gst::ElementFactory::find(enc).is_none() {
                continue;
            }
            out.push(Candidate {
                max_fps: cfg.max_fps,
                label: "va-dmabuf",
                desc: format!(
                    "{src} ! video/x-raw(memory:DMABuf) ! vapostproc ! video/x-raw(memory:VAMemory),format=NV12 ! {enc} name=enc {va_opts} ! {parse} ! {caps} ! {sink}"
                ),
            });
            out.push(Candidate {
                max_fps: cfg.max_fps,
                label: "va-sysmem",
                desc: format!(
                    "{src} ! videoconvert ! vapostproc ! video/x-raw(memory:VAMemory),format=NV12 ! {enc} name=enc {va_opts} ! {parse} ! {caps} ! {sink}"
                ),
            });
        }
    }
    if cfg.codec == Codec::H264 && gst::ElementFactory::find("x264enc").is_some() {
        out.push(Candidate {
            max_fps: cfg.max_fps,
            label: "x264",
            desc: format!(
                "{src} ! videoconvert ! video/x-raw,format=I420 ! x264enc name=enc tune=zerolatency speed-preset=ultrafast \
                 bitrate={kbps} key-int-max={x264_gop} bframes=0 sliced-threads=true aud={aud}{x264_cpb} ! {parse} ! {caps} ! {sink}"
            ),
        });
    }
    out
}

/// `cpb-size` (kilobits) holding `frames` frames at `kbps`.
fn cpb_size_kb(kbps: u32, frames: u32, fps: u32) -> u32 {
    (u64::from(kbps) * u64::from(frames) / u64::from(fps.max(1))).max(1) as u32
}

struct Shared {
    tx: mpsc::Sender<EncodedFrame>,
    /// Set when frames had to be dropped; non-keyframes are skipped until the next keyframe.
    resync: AtomicBool,
    frames: AtomicU64,
    bytes: AtomicU64,
    errors: Mutex<Option<mpsc::UnboundedSender<String>>>,
    /// Buffers that went into the encoder, in order, to match its output against.
    captured: Mutex<std::collections::VecDeque<Captured>>,
}

/// A buffer that passed the capture probe.
#[derive(Debug, Clone, Copy)]
struct Captured {
    pts: gst::ClockTime,
    /// Keepalive repeat of the previous frame.
    repeat: bool,
    captured_us: u64,
}

/// Matches encoder output to captured buffers. The encoder emits one frame per input, in
/// order, and keeps the spacing of the timestamps (it moves them by a constant offset), so
/// the next entry whose distance from the last matched input equals the output's distance
/// from the last matched output is the frame; anything skipped before it was dropped.
#[derive(Debug, Default)]
struct OutputMatcher {
    last: Option<(gst::ClockTime, gst::ClockTime)>,
}

impl OutputMatcher {
    fn matches(
        &mut self,
        q: &mut std::collections::VecDeque<Captured>,
        out_pts: Option<gst::ClockTime>,
    ) -> Option<Captured> {
        let Some(out) = out_pts else {
            return q.pop_front();
        };
        let tolerance = gst::ClockTime::from_useconds(500).nseconds();
        let found = match self.last {
            None => q.pop_front(),
            Some((in_prev, out_prev)) => {
                let want = out.saturating_sub(out_prev).nseconds();
                let pos = q.iter().position(|c| {
                    let d = c.pts.saturating_sub(in_prev).nseconds();
                    d.abs_diff(want) <= tolerance
                });
                match pos {
                    Some(i) => {
                        q.drain(..i);
                        q.pop_front()
                    }
                    // Lost sync: start over from the oldest entry.
                    None => q.pop_front(),
                }
            }
        };
        if let Some(c) = found {
            self.last = Some((c.pts, out));
        }
        found
    }
}

/// Tells pipewiresrc's keepalive repeats from real frames. A repeat is a `gst_buffer_copy` of
/// a copy of the last real buffer, and every copy carries the previous buffer along as a
/// `ParentBufferMeta`, so repeats have more of those metas than real frames (whose count is
/// taken from the first buffer, which is always real).
#[derive(Debug, Default)]
struct RepeatDetector {
    baseline: Option<usize>,
}

impl RepeatDetector {
    fn is_repeat(&mut self, buffer: &gst::BufferRef) -> bool {
        let n = buffer.iter_meta::<gst::meta::ParentBufferMeta>().count();
        match self.baseline {
            Some(b) => n > b,
            None => {
                self.baseline = Some(n);
                false
            }
        }
    }
}

/// A running capture + encode pipeline. Dropping it stops the pipeline.
pub struct Encoder {
    pipeline: gst::Pipeline,
    sink: gst_app::AppSink,
    enc: gst::Element,
    shared: Arc<Shared>,
    label: &'static str,
    /// (`cpb_frames`, fps) when the CPB is capped, so it follows bitrate changes.
    cpb: Option<(u32, u32)>,
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
            match Self::try_start(&cand, cfg) {
                Ok(v) => {
                    info!(
                        label = cand.label,
                        width = cfg.width,
                        height = cfg.height,
                        kbps = cfg.bitrate_kbps,
                        target_usage = cfg.target_usage,
                        fillers = cfg.filler_frames,
                        cpb_frames = cfg.cpb_frames,
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

    fn try_start(
        cand: &Candidate,
        cfg: &EncoderConfig,
    ) -> Result<(Encoder, EncoderOutputs), String> {
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
            captured: Mutex::new(std::collections::VecDeque::new()),
        });

        // Capture probe: frame-rate limit on real frames, and a record of what goes into the
        // encoder (for capture → encoded timing and to flag fillers). Without fillers,
        // keepalive repeats count like frames (they only keep the cadence when the screen is
        // idle). With fillers they bypass the limit and are flagged so the tablet decodes but
        // doesn't show them.
        {
            let limiter = Mutex::new(FrameLimiter::new(cand.max_fps.max(1)));
            let detector = Mutex::new(RepeatDetector::default());
            let limit = cand.max_fps > 0 && cand.max_fps < 240;
            let fillers = cfg.filler_frames > 0;
            let shared = shared.clone();
            let pad = pipeline
                .by_name("src")
                .and_then(|e| e.static_pad("src"))
                .ok_or("no source pad")?;
            pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                let Some(buffer) = info.buffer() else {
                    return gst::PadProbeReturn::Ok;
                };
                let repeat = detector
                    .lock()
                    .map(|mut d| d.is_repeat(buffer))
                    .unwrap_or(false);
                let keep = (fillers && repeat)
                    || !limit
                    || limiter
                        .lock()
                        .map(|mut l| l.admit(Instant::now()))
                        .unwrap_or(true);
                if !keep {
                    return gst::PadProbeReturn::Drop;
                }
                if let (Some(pts), Ok(mut q)) = (buffer.pts(), shared.captured.lock()) {
                    q.push_back(Captured {
                        pts,
                        repeat: fillers && repeat,
                        captured_us: now_us(),
                    });
                    if q.len() > 256 {
                        q.pop_front();
                    }
                }
                gst::PadProbeReturn::Ok
            });
        }

        {
            let shared = shared.clone();
            let mut matcher = OutputMatcher::default();
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
                        let encoded = now_us();
                        let captured = shared
                            .captured
                            .lock()
                            .ok()
                            .and_then(|mut q| matcher.matches(&mut q, buffer.pts()));
                        let filler = captured.is_some_and(|c| c.repeat) && !keyframe;
                        let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                        let frame = EncodedFrame {
                            data: map.as_slice().to_vec(),
                            keyframe,
                            filler,
                            capture_time_us: captured.map_or(encoded, |c| c.captured_us),
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
            cpb: (cfg.cpb_frames > 0).then_some((cfg.cpb_frames, cfg.max_fps.max(1))),
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
        if let Some((frames, fps)) = self.cpb {
            if self.enc.has_property("cpb-size") {
                self.enc
                    .set_property("cpb-size", cpb_size_kb(kbps, frames, fps));
            }
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

/// Measures the hardware encoder's speed: encodes `frames` repeats of a test picture at
/// `width`×`height` as fast as it can and returns frames per second (`None` without a VA
/// encoder for `codec`). Used by `slated doctor` to show the budget for filler frames.
pub fn benchmark_encoder(
    codec: Codec,
    width: u32,
    height: u32,
    target_usage: u32,
    frames: u32,
) -> Option<f64> {
    init().ok()?;
    let enc = match codec {
        Codec::H264 => "vah264enc",
        Codec::Hevc => "vah265enc",
    };
    gst::ElementFactory::find(enc)?;
    let kbps = default_bitrate_kbps(width, height, 60);
    let desc = format!(
        "videotestsrc num-buffers=1 pattern=ball ! video/x-raw,width={width},height={height},format=NV12 \
         ! imagefreeze num-buffers={frames} ! video/x-raw,framerate=240/1 \
         ! vapostproc ! video/x-raw(memory:VAMemory),format=NV12 \
         ! {enc} rate-control=cbr bitrate={kbps} key-int-max=1024 b-frames=0 ref-frames=1 target-usage={target_usage} \
         ! fakesink sync=false"
    );
    let pipeline = gst::parse::launch(&desc).ok()?;
    let bus = pipeline.bus()?;
    pipeline.set_state(gst::State::Playing).ok()?;
    let start = Instant::now();
    let msg = bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(30),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    let secs = start.elapsed().as_secs_f64();
    let _ = pipeline.set_state(gst::State::Null);
    match msg.map(|m| m.type_()) {
        Some(gst::MessageType::Eos) if secs > 0.0 => Some(f64::from(frames) / secs),
        _ => None,
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
    fn frame_limiter_averages_to_target() {
        // 60 Hz source for 10 s.
        for (target, expect) in [
            (30u32, 300usize),
            (24, 240),
            (20, 200),
            (45, 450),
            (60, 600),
            (15, 150),
        ] {
            let mut l = FrameLimiter::new(target);
            let t0 = Instant::now();
            let kept = (0..600)
                .filter(|i| l.admit(t0 + Duration::from_micros(i * 16_667)))
                .count();
            let tol = expect / 20 + 2;
            assert!(
                kept.abs_diff(expect) <= tol,
                "target {target}: kept {kept}, expected ~{expect}"
            );
        }
        // No burst after an idle gap.
        let mut l = FrameLimiter::new(30);
        let t0 = Instant::now();
        assert!(l.admit(t0));
        let later = t0 + Duration::from_secs(5);
        assert!(l.admit(later));
        assert!(!l.admit(later + Duration::from_millis(16)));
    }

    #[test]
    fn frame_spacing_with_fillers() {
        let mut c = EncoderConfig::new("x", 1920, 1200);
        c.max_fps = 60;
        assert_eq!(c.frame_spacing(), Duration::from_millis(16));
        c.filler_frames = 1;
        assert_eq!(c.frame_spacing(), Duration::from_millis(8));
        c.filler_frames = 3;
        assert_eq!(c.frame_spacing(), Duration::from_millis(4));
        c.max_fps = 240;
        c.filler_frames = 10;
        assert_eq!(c.frame_spacing(), Duration::from_millis(2));
        assert_eq!(cpb_size_kb(20_000, 2, 60), 666);
    }

    #[test]
    fn pipeline_options_are_rendered() {
        if init().is_err() {
            return;
        }
        let mut c = EncoderConfig::new("x", 1920, 1200);
        c.codec = Codec::Hevc;
        c.target_usage = 3;
        c.cpb_frames = 1;
        c.aud = true;
        c.bitrate_kbps = 12_000;
        let descs: Vec<String> = candidates(&c).into_iter().map(|c| c.desc).collect();
        // Without gst-plugin-va there may be no candidates for HEVC; the format is still
        // checked where there are.
        for d in &descs {
            assert!(d.contains("target-usage=3"), "{d}");
            assert!(d.contains("key-int-max=1024"), "{d}");
            assert!(d.contains("cpb-size=200"), "{d}");
            assert!(d.contains("aud=true"), "{d}");
        }
        c.gop_frames = 300;
        for d in candidates(&c).into_iter().map(|c| c.desc) {
            assert!(d.contains("key-int-max=300"), "{d}");
        }
    }

    #[test]
    fn output_matcher_follows_spacing_and_drops() {
        use std::collections::VecDeque;
        let ms = |v: u64| gst::ClockTime::from_mseconds(v);
        let cap = |pts, repeat| Captured {
            pts: ms(pts),
            repeat,
            captured_us: pts * 1000,
        };
        let mut q: VecDeque<Captured> = [
            cap(0, false),
            cap(5, true),
            cap(10, true),
            cap(16, false),
            cap(21, true),
        ]
        .into();
        let mut m = OutputMatcher::default();
        // The encoder shifts timestamps by a constant (here 1000 h).
        let shift = gst::ClockTime::from_seconds(1000 * 3600);
        assert!(!m.matches(&mut q, Some(ms(0) + shift)).unwrap().repeat);
        assert!(m.matches(&mut q, Some(ms(5) + shift)).unwrap().repeat);
        // The encoder dropped the 10 ms repeat: the 16 ms frame is matched by its distance.
        let c = m.matches(&mut q, Some(ms(16) + shift)).unwrap();
        assert_eq!((c.pts, c.repeat), (ms(16), false));
        assert_eq!(c.captured_us, 16_000);
        assert!(m.matches(&mut q, Some(ms(21) + shift)).unwrap().repeat);
        assert!(m.matches(&mut q, Some(ms(30) + shift)).is_none());
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
                "rate-control=cbr bitrate=8000 key-int-max=1024 b-frames=0 ref-frames=1 target-usage=1 aud=true cpb-size=266"
            } else {
                "tune=zerolatency speed-preset=ultrafast bitrate=8000 key-int-max=1073741824 bframes=0 aud=true vbv-buf-capacity=33"
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
