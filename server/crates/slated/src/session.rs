//! One connected tablet: handshake, pairing, virtual output, video stream, touch.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slate_kwin::eis::TouchAction;
use slate_protocol::framing::{read_frame, write_frame, FrameError};
use slate_protocol::pb::{self, ErrorCode};
use slate_protocol::touch::Rect;
use slate_protocol::{error, Envelope, Msg, PROTOCOL_VERSION};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::backend::{Backends, Codec, EncoderConfig, Geometry};
use crate::config::Config;
use crate::store::Store;

/// Touch ids are namespaced per session: `slot * TOUCH_IDS_PER_SLOT + android_pointer_id`.
pub const TOUCH_IDS_PER_SLOT: u32 = 64;
/// Maximum concurrent sessions supported by the touch id namespace.
pub const MAX_SLOTS: u32 = 16;

/// Shared daemon state given to every session.
pub struct SessionContext {
    pub backends: Backends,
    pub store: Arc<Mutex<Store>>,
    pub config: Arc<Config>,
    pub server_name: String,
    pub fingerprint_short: String,
    pub registry: Arc<Registry>,
}

impl std::fmt::Debug for SessionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionContext")
            .field("server_name", &self.server_name)
            .finish_non_exhaustive()
    }
}

/// Live statistics of a session (shown in the tray app).
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub frames_sent: u64,
    pub bytes_sent: u64,
    pub frames_dropped: u64,
    pub rtt_us: u32,
    pub client_decode_us: u32,
    pub client_frames_dropped: u32,
    pub latency_us: u32,
    pub bitrate_kbps: u32,
}

/// Public information about a running session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub device_id: String,
    pub device_name: String,
    pub output_name: String,
    pub peer: String,
    pub link: String,
    pub width: u32,
    pub height: u32,
    pub encoder: String,
    pub codec: String,
    pub started_unix: u64,
    pub slot: u32,
    /// Stream fps actually used.
    pub fps: u32,
    /// Tablet panel size / refresh (upper bounds for settings).
    pub native_width: u32,
    pub native_height: u32,
    pub panel_fps: u32,
    /// Settings in effect (as configured: `native`, `75%`, `1280x800`; fps 0 = auto).
    pub settings: crate::stream_settings::StreamSettings,
    pub cancel: CancellationToken,
    /// Set before cancelling to ask the client to reconnect (settings changed).
    pub reconnect: Arc<std::sync::atomic::AtomicBool>,
    pub stats: SessionStats,
}

/// All running sessions.
#[derive(Debug)]
pub struct Registry {
    sessions: Mutex<HashMap<String, SessionInfo>>,
    pub changed: broadcast::Sender<()>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            changed: broadcast::channel(16).0,
        }
    }
}

impl Registry {
    pub fn list(&self) -> Vec<SessionInfo> {
        let mut v: Vec<_> = self
            .sessions
            .lock()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default();
        v.sort_by_key(|s| s.started_unix);
        v
    }

    pub fn len(&self) -> usize {
        self.sessions.lock().map(|g| g.len()).unwrap_or(0)
    }

    fn free_slot(&self) -> Option<u32> {
        let g = self.sessions.lock().ok()?;
        (0..MAX_SLOTS).find(|s| !g.values().any(|i| i.slot == *s))
    }

    fn insert(&self, info: SessionInfo) {
        if let Ok(mut g) = self.sessions.lock() {
            g.insert(info.session_id.clone(), info);
        }
        let _ = self.changed.send(());
    }

    fn update(&self, session_id: &str, f: impl FnOnce(&mut SessionInfo)) {
        if let Ok(mut g) = self.sessions.lock() {
            if let Some(i) = g.get_mut(session_id) {
                f(i);
            }
        }
    }

    fn remove(&self, session_id: &str) {
        if let Ok(mut g) = self.sessions.lock() {
            g.remove(session_id);
        }
        let _ = self.changed.send(());
    }

    /// Cancels a session by id. Returns whether it existed.
    pub fn cancel(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .ok()
            .and_then(|g| g.get(session_id).map(|s| s.cancel.cancel()))
            .is_some()
    }

    /// Ends the device's sessions asking the tablet to reconnect (to apply new settings).
    pub fn reconnect_device(&self, device_id: &str) -> usize {
        let Ok(g) = self.sessions.lock() else {
            return 0;
        };
        let mut n = 0;
        for s in g.values().filter(|s| s.device_id == device_id) {
            s.reconnect.store(true, std::sync::atomic::Ordering::SeqCst);
            s.cancel.cancel();
            n += 1;
        }
        n
    }

    /// Cancels every session of a device; returns their ids.
    pub fn cancel_device(&self, device_id: &str) -> Vec<String> {
        let Ok(g) = self.sessions.lock() else {
            return Vec::new();
        };
        g.values()
            .filter(|s| s.device_id == device_id)
            .map(|s| {
                s.cancel.cancel();
                s.session_id.clone()
            })
            .collect()
    }

    pub fn cancel_all(&self) {
        if let Ok(g) = self.sessions.lock() {
            for s in g.values() {
                s.cancel.cancel();
            }
        }
    }

    fn contains(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .map(|g| g.contains_key(session_id))
            .unwrap_or(false)
    }
}

/// Errors that end a session and are reported to the client.
#[derive(Debug)]
struct Fatal(ErrorCode, String);

impl Fatal {
    fn new(code: ErrorCode, msg: impl Into<String>) -> Self {
        Self(code, msg.into())
    }
}

impl From<anyhow::Error> for Fatal {
    fn from(e: anyhow::Error) -> Self {
        Fatal(ErrorCode::Internal, e.to_string())
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Runs a session to completion on an established (TLS) stream.
pub async fn run<S>(stream: S, peer: String, ctx: Arc<SessionContext>, shutdown: CancellationToken)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (rd, wr) = tokio::io::split(stream);
    let (ctl_tx, ctl_rx) = mpsc::channel::<Envelope>(64);
    let (vid_tx, vid_rx) = mpsc::channel::<Envelope>(3);
    let writer = tokio::spawn(writer_task(wr, ctl_rx, vid_rx));
    let (in_tx, mut inbox) = mpsc::channel::<Result<Envelope, FrameError>>(64);
    let reader = tokio::spawn(reader_task(rd, in_tx));

    let mut s = Session {
        ctx: ctx.clone(),
        peer: peer.clone(),
        ctl: ctl_tx.clone(),
        vid: vid_tx,
        session_id: None,
        slot: None,
    };
    let result = s.body(&mut inbox, shutdown).await;
    match &result {
        Ok(reason) => info!(%peer, reason, "session ended"),
        Err(Fatal(code, msg)) => {
            warn!(%peer, ?code, msg, "session failed");
            let _ = ctl_tx.send(error(*code, msg.clone())).await;
        }
    }
    s.cleanup();
    drop(s);
    drop(ctl_tx);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    reader.abort();
}

async fn reader_task<R: AsyncRead + Unpin>(
    mut rd: R,
    tx: mpsc::Sender<Result<Envelope, FrameError>>,
) {
    loop {
        let r = read_frame(&mut rd).await;
        let end = r.is_err();
        if tx.send(r).await.is_err() || end {
            break;
        }
    }
}

async fn writer_task<W: AsyncWrite + Unpin>(
    mut wr: W,
    mut ctl: mpsc::Receiver<Envelope>,
    mut vid: mpsc::Receiver<Envelope>,
) {
    let mut ctl_open = true;
    let mut vid_open = true;
    while ctl_open || vid_open {
        let env = tokio::select! {
            biased;
            m = ctl.recv(), if ctl_open => match m { Some(m) => m, None => { ctl_open = false; continue; } },
            m = vid.recv(), if vid_open => match m { Some(m) => m, None => { vid_open = false; continue; } },
        };
        if let Err(e) = write_frame(&mut wr, &env).await {
            debug!("writer stopped: {e}");
            break;
        }
        // An orderly error/goodbye ends the stream.
        if matches!(env.msg, Some(Msg::Error(_)) | Some(Msg::Goodbye(_))) && !ctl_open {
            break;
        }
    }
    let _ = tokio::io::AsyncWriteExt::shutdown(&mut wr).await;
}

struct Session {
    ctx: Arc<SessionContext>,
    peer: String,
    ctl: mpsc::Sender<Envelope>,
    vid: mpsc::Sender<Envelope>,
    session_id: Option<String>,
    slot: Option<u32>,
}

/// Validated parameters for the stream.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamPlan {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Largest size the tablet supports (panel, limited by its decoder).
    pub native_width: u32,
    pub native_height: u32,
    /// Tablet refresh rate (limited by its decoder).
    pub panel_fps: u32,
}

impl StreamPlan {
    /// Applies resolution / refresh-rate settings (never above the native values).
    pub fn with_settings(mut self, s: &crate::stream_settings::StreamSettings) -> Self {
        let (w, h) = s.resolution().apply(self.native_width, self.native_height);
        self.width = w;
        self.height = h;
        self.fps = s.effective_fps(self.panel_fps);
        self
    }
}

/// Chooses codec and stream size from the client's capabilities (landscape only).
/// `prefer_hevc`: `Some` forces a preference, `None` follows the client's decoder order
/// (clients list their lowest-latency decoder first).
pub fn plan_stream(
    hello: &pb::ClientHello,
    prefer_hevc: Option<bool>,
) -> Result<StreamPlan, (ErrorCode, String)> {
    let d = hello
        .display
        .as_ref()
        .ok_or((ErrorCode::BadMessage, "missing display info".to_string()))?;
    if d.width_px == 0 || d.height_px == 0 || d.width_px > 8192 || d.height_px > 8192 {
        return Err((
            ErrorCode::BadMessage,
            format!("bad display size {}x{}", d.width_px, d.height_px),
        ));
    }
    let find = |c: pb::Codec| hello.decoders.iter().find(|x| x.codec == c as i32);
    let first = hello
        .decoders
        .iter()
        .find(|d| d.codec == pb::Codec::H264 as i32 || d.codec == pb::Codec::Hevc as i32);
    let as_plan = |d: &pb::DecoderInfo| {
        if d.codec == pb::Codec::Hevc as i32 {
            Codec::Hevc
        } else {
            Codec::H264
        }
    };
    let (codec, dec) = match (prefer_hevc, find(pb::Codec::H264), find(pb::Codec::Hevc)) {
        (Some(true), _, Some(h)) => (Codec::Hevc, h),
        (Some(false), Some(a), _) => (Codec::H264, a),
        _ => match first {
            Some(d) => (as_plan(d), d),
            None => {
                return Err((
                    ErrorCode::NoCommonCodec,
                    "client offers no H.264/HEVC decoder".to_string(),
                ))
            }
        },
    };
    // Landscape: long side horizontal.
    let (mut w, mut h) = (d.width_px.max(d.height_px), d.width_px.min(d.height_px));
    let (mw, mh) = (
        dec.max_width.max(dec.max_height),
        dec.max_width.min(dec.max_height),
    );
    if mw > 0 && mh > 0 && (w > mw || h > mh) {
        let f = (f64::from(mw) / f64::from(w)).min(f64::from(mh) / f64::from(h));
        w = ((f64::from(w) * f) as u32) & !1;
        h = ((f64::from(h) * f) as u32) & !1;
    }
    let refresh = if d.refresh_hz.is_finite() && d.refresh_hz > 1.0 {
        d.refresh_hz.round() as u32
    } else {
        60
    };
    let mut fps = refresh.clamp(24, 60);
    if dec.max_fps > 0 {
        fps = fps.min(dec.max_fps);
    }
    Ok(StreamPlan {
        codec,
        width: w & !1,
        height: h & !1,
        fps,
        native_width: w & !1,
        native_height: h & !1,
        panel_fps: fps,
    })
}

/// `PlasmaSlate-<first 8 alphanumerics of the device id>`.
pub fn output_base_name(device_id: &str) -> String {
    let short: String = device_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect();
    format!(
        "PlasmaSlate-{}",
        if short.is_empty() {
            "tablet".into()
        } else {
            short.to_lowercase()
        }
    )
}

impl Session {
    async fn send(&self, env: Envelope) -> Result<(), Fatal> {
        self.ctl
            .send(env)
            .await
            .map_err(|_| Fatal::new(ErrorCode::Internal, "connection closed"))
    }

    async fn expect_hello(
        &self,
        inbox: &mut mpsc::Receiver<Result<Envelope, FrameError>>,
    ) -> Result<pb::ClientHello, Fatal> {
        match tokio::time::timeout(Duration::from_secs(10), inbox.recv()).await {
            Ok(Some(Ok(Envelope {
                msg: Some(Msg::ClientHello(h)),
            }))) => Ok(h),
            Ok(Some(Ok(other))) => Err(Fatal::new(
                ErrorCode::BadMessage,
                format!("expected ClientHello, got {}", other.kind()),
            )),
            Ok(Some(Err(e))) => Err(Fatal::new(ErrorCode::BadMessage, e.to_string())),
            Ok(None) => Err(Fatal::new(ErrorCode::BadMessage, "closed before hello")),
            Err(_) => Err(Fatal::new(ErrorCode::BadMessage, "no hello within 10 s")),
        }
    }

    async fn body(
        &mut self,
        inbox: &mut mpsc::Receiver<Result<Envelope, FrameError>>,
        shutdown: CancellationToken,
    ) -> Result<&'static str, Fatal> {
        let ctx = self.ctx.clone();
        let hello = self.expect_hello(inbox).await?;
        if hello.protocol_version != PROTOCOL_VERSION {
            return Err(Fatal::new(
                ErrorCode::ProtocolVersion,
                format!(
                    "server speaks protocol {PROTOCOL_VERSION}, client {}",
                    hello.protocol_version
                ),
            ));
        }
        let device_id: String = hello
            .device_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(64)
            .collect();
        if device_id.len() < 8 {
            return Err(Fatal::new(ErrorCode::BadMessage, "invalid device id"));
        }
        let device_name: String = hello
            .device_name
            .chars()
            .filter(|c| !c.is_control())
            .take(64)
            .collect();
        let link = match pb::Link::try_from(hello.link).unwrap_or(pb::Link::Unspecified) {
            pb::Link::Usb => "usb",
            pb::Link::Network => "network",
            pb::Link::Unspecified => "unknown",
        };
        info!(peer = %self.peer, %device_id, %device_name, link, app = %hello.app_version, "client hello");

        // --- authentication / pairing
        let authorized = ctx
            .store
            .lock()
            .map(|s| s.verify(&device_id, &hello.auth_token))
            .unwrap_or(false);
        let mut issued_token = String::new();
        if !authorized {
            self.send(Envelope::new(Msg::PairingPending(pb::PairingPending {
                message: format!("Confirm on {} to continue", ctx.server_name),
                server_fingerprint: ctx.fingerprint_short.clone(),
            })))
            .await?;
            let approved = ctx.config.auto_approve
                || tokio::select! {
                    a = ctx.backends.approver.approve(&device_name, &device_id, &ctx.fingerprint_short) => a,
                    _ = shutdown.cancelled() => false,
                };
            if !approved {
                return Err(Fatal::new(
                    ErrorCode::PairingDenied,
                    "pairing was not accepted on the PC",
                ));
            }
            issued_token = ctx
                .store
                .lock()
                .map_err(|_| Fatal::new(ErrorCode::Internal, "store poisoned"))?
                .pair(&device_id, &device_name)
                .map_err(Fatal::from)?;
        } else if let Ok(mut s) = ctx.store.lock() {
            let _ = s.touch(&device_id, &device_name);
        }

        // --- one session per device: replace an older one (reconnect after Wi-Fi drop).
        let replaced = ctx.registry.cancel_device(&device_id);
        if !replaced.is_empty() {
            info!(%device_id, "replacing previous session");
            for _ in 0..50 {
                if !replaced.iter().any(|id| ctx.registry.contains(id)) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        if ctx.registry.len() >= ctx.config.max_sessions {
            return Err(Fatal::new(
                ErrorCode::Internal,
                "too many tablets connected",
            ));
        }
        let slot = ctx
            .registry
            .free_slot()
            .ok_or_else(|| Fatal::new(ErrorCode::Internal, "no free session slot"))?;

        // --- stream plan + virtual output
        let settings = ctx
            .store
            .lock()
            .ok()
            .and_then(|s| s.stream_settings(&device_id))
            .unwrap_or_else(|| ctx.config.default_stream());
        let plan = plan_stream(&hello, ctx.config.codec_preference())
            .map_err(|(c, m)| Fatal::new(c, m))?
            .with_settings(&settings);
        let display = hello.display.unwrap_or_default();
        let dpi = if display.density_dpi > 0 {
            f64::from(display.density_dpi)
        } else if display.physical_width_mm > 0.0 {
            f64::from(display.width_px.max(display.height_px))
                / (f64::from(display.physical_width_mm.max(display.physical_height_mm)) / 25.4)
        } else {
            160.0
        };
        // Density of the (possibly reduced) stream as seen on the panel, for the default scale.
        let dpi = dpi * f64::from(plan.width) / f64::from(plan.native_width.max(1));
        let base = output_base_name(&device_id);
        let description = if device_name.is_empty() {
            "Plasma Slate tablet".to_string()
        } else {
            device_name.clone()
        };
        let mut output = ctx
            .backends
            .outputs
            .create(&base, &description, plan.width, plan.height)
            .await
            .map_err(|e| Fatal::new(ErrorCode::OutputFailed, e.to_string()))?;
        let output_name = output.output_name().to_string();
        let saved = ctx.store.lock().ok().and_then(|s| s.placement(&device_id));
        let placement = ctx
            .backends
            .outputs
            .configure(
                &output_name,
                plan.width,
                plan.height,
                f64::from(plan.fps),
                saved,
                dpi,
            )
            .await
            .map_err(|e| Fatal::new(ErrorCode::OutputFailed, e.to_string()))?;
        if let Ok(mut s) = ctx.store.lock() {
            let _ = s.set_placement(&device_id, placement);
        }
        // Offer the preset sizes and refresh rates in Display & Monitor settings; picking one
        // there is picked up below and applied like a change made in Plasma Slate.
        let modes = crate::stream_settings::display_modes(
            plan.native_width,
            plan.native_height,
            plan.panel_fps,
        );
        if let Err(e) = ctx
            .backends
            .outputs
            .ensure_modes(&output_name, &modes)
            .await
        {
            warn!(%output_name, "could not add display modes: {e}");
        }

        let session_id = uuid::Uuid::new_v4().to_string();
        let cancel = shutdown.child_token();
        let reconnect = Arc::new(std::sync::atomic::AtomicBool::new(false));
        ctx.registry.insert(SessionInfo {
            session_id: session_id.clone(),
            device_id: device_id.clone(),
            device_name: device_name.clone(),
            output_name: output_name.clone(),
            peer: self.peer.clone(),
            link: link.to_string(),
            width: plan.width,
            height: plan.height,
            encoder: String::new(),
            codec: match plan.codec {
                Codec::H264 => "H.264".to_string(),
                Codec::Hevc => "HEVC".to_string(),
            },
            started_unix: unix_now(),
            slot,
            fps: plan.fps,
            native_width: plan.native_width,
            native_height: plan.native_height,
            panel_fps: plan.panel_fps,
            settings: settings.clone(),
            cancel: cancel.clone(),
            reconnect: reconnect.clone(),
            stats: SessionStats::default(),
        });
        self.session_id = Some(session_id.clone());
        self.slot = Some(slot);

        let bitrate = ctx.config.bitrate_kbps.unwrap_or_else(|| {
            slate_media::default_bitrate_kbps(plan.width, plan.height, plan.fps)
        });
        let stream_cfg = pb::StreamConfig {
            codec: match plan.codec {
                Codec::H264 => pb::Codec::H264 as i32,
                Codec::Hevc => pb::Codec::Hevc as i32,
            },
            width: plan.width,
            height: plan.height,
            max_fps: plan.fps,
            bitrate_kbps: bitrate,
        };
        self.send(Envelope::new(Msg::ServerHello(pb::ServerHello {
            protocol_version: PROTOCOL_VERSION,
            session_id: session_id.clone(),
            server_name: ctx.server_name.clone(),
            output_name: output_name.clone(),
            stream: Some(stream_cfg),
            issued_auth_token: issued_token,
        })))
        .await?;

        // --- wait for the decoder to be ready
        let ready = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                match inbox.recv().await {
                    Some(Ok(Envelope {
                        msg: Some(Msg::Ready(_)),
                    })) => return Ok(()),
                    Some(Ok(Envelope {
                        msg: Some(Msg::Ping(p)),
                    })) => {
                        let _ = self.ctl.send(pong(&p)).await;
                    }
                    Some(Ok(Envelope {
                        msg: Some(Msg::Goodbye(_)),
                    }))
                    | None => return Err(Fatal::new(ErrorCode::BadMessage, "closed before ready")),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(Fatal::new(ErrorCode::BadMessage, e.to_string())),
                }
            }
        })
        .await
        .map_err(|_| Fatal::new(ErrorCode::BadMessage, "client did not become ready"))?;
        ready?;

        // --- encoder
        let mut enc_cfg = EncoderConfig::new(output.pipewire_target(), plan.width, plan.height);
        enc_cfg.codec = plan.codec;
        enc_cfg.max_fps = plan.fps;
        // Keep a steady frame cadence even when the screen is idle: hardware decoders hold a
        // couple of frames, so with damage-only frames the last change would sit in the decoder
        // until something else moves. Repeated frames encode to a few hundred bytes.
        enc_cfg.keepalive = Duration::from_millis(u64::from(1000 / plan.fps.max(1)) + 4);
        enc_cfg.bitrate_kbps = bitrate;
        enc_cfg.software_only = ctx.config.software_encoding;
        let encoders = ctx.backends.encoders.clone();
        let started = tokio::task::spawn_blocking(move || encoders.start(&enc_cfg))
            .await
            .map_err(|e| Fatal::new(ErrorCode::EncoderFailed, e.to_string()))?
            .map_err(|e| Fatal::new(ErrorCode::EncoderFailed, e.to_string()))?;
        let variant = started.handle.variant();
        ctx.registry
            .update(&session_id, |i| i.encoder = variant.to_string());
        let _ = ctx.registry.changed.send(());
        info!(%device_name, %output_name, w = plan.width, h = plan.height, fps = plan.fps, kbps = bitrate, encoder = variant, "streaming");

        let crate::backend::StartedEncoder {
            handle: encoder,
            mut frames,
            mut errors,
        } = started;
        let mut sequence: u64 = 0;
        let mut waiting_for_key = false;
        let mut last_rx = Instant::now();
        let initial = ctx
            .backends
            .outputs
            .query(&output_name)
            .await
            .ok()
            .flatten();
        let mut geometry: Option<Geometry> = initial.map(|i| i.geometry);
        // The mode chosen for this stream; a different one means the user picked another mode
        // in Display & Monitor settings.
        let mut expected_mode = initial.and_then(|i| i.mode);
        let mut ping = tokio::time::interval(Duration::from_secs(2));
        let mut placement_tick = tokio::time::interval(Duration::from_secs(2));
        let mut ping_id: u64 = 0;
        let mut touch_warned = false;
        let mut rate = BitrateController::new(bitrate);
        let mut dropped_seen: u64 = 0;
        let (mut stats_ticks, mut last_frames, mut last_bytes) = (0u64, 0u64, 0u64);
        // Startup (big first keyframe, decoder init) looks like congestion; let it settle first.
        let streaming_since = Instant::now();
        ctx.registry
            .update(&session_id, |i| i.stats.bitrate_kbps = bitrate);

        let reason = loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    let again = reconnect.load(std::sync::atomic::Ordering::SeqCst);
                    let reason = if again { "applying new display settings" } else { "disconnected on the PC" };
                    let _ = self.ctl.send(Envelope::new(Msg::Goodbye(pb::Goodbye { reason: reason.into(), reconnect: again }))).await;
                    break if again { "reconfiguring" } else { "cancelled" };
                }
                _ = output.closed() => {
                    return Err(Fatal::new(ErrorCode::OutputFailed, "the virtual screen was removed"));
                }
                m = inbox.recv() => {
                    last_rx = Instant::now();
                    match m {
                        None | Some(Err(FrameError::Closed)) => break "client disconnected",
                        Some(Err(FrameError::Io(e))) if matches!(
                            e.kind(),
                            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionAborted
                        ) => break "client disconnected",
                        Some(Err(e)) => return Err(Fatal::new(ErrorCode::BadMessage, e.to_string())),
                        Some(Ok(env)) => match env.msg {
                            Some(Msg::TouchEvent(t)) => {
                                if geometry.is_none() {
                                    geometry = ctx.backends.outputs.query(&output_name).await.ok().flatten().map(|i| i.geometry);
                                }
                                let region = ctx.backends.touch.region(&output_name).or(geometry);
                                if let Some(region) = region {
                                    let actions = touch_actions(&t, slot, region);
                                    if let Err(e) = ctx.backends.touch.send(&actions) {
                                        if !touch_warned {
                                            warn!("touch injection failed: {e}");
                                            touch_warned = true;
                                        }
                                    }
                                }
                            }
                            Some(Msg::Ping(p)) => { let _ = self.ctl.send(pong(&p)).await; }
                            Some(Msg::Pong(p)) => {
                                let rtt = slate_media::now_us().saturating_sub(p.ping_sender_time_us);
                                ctx.registry.update(&session_id, |i| i.stats.rtt_us = u32::try_from(rtt).unwrap_or(u32::MAX));
                            }
                            Some(Msg::RequestKeyframe(r)) => {
                                debug!(reason = %r.reason, "client requested keyframe");
                                encoder.force_keyframe();
                            }
                            Some(Msg::ClientStats(s)) => {
                                let mut dropped_total = 0;
                                ctx.registry.update(&session_id, |i| {
                                    i.stats.client_decode_us = s.avg_decode_us;
                                    i.stats.client_frames_dropped = s.frames_dropped;
                                    i.stats.latency_us = s.avg_latency_us;
                                    dropped_total = i.stats.frames_dropped;
                                });
                                let dropped_now = dropped_total.saturating_sub(dropped_seen);
                                dropped_seen = dropped_total;
                                if ctx.config.bitrate_kbps.is_none() && streaming_since.elapsed() > Duration::from_secs(5) {
                                    if let Some(kbps) = rate.update(dropped_now, s.rtt_us) {
                                        info!(kbps, rtt_us = s.rtt_us, dropped = dropped_now, "adaptive bitrate");
                                        encoder.set_bitrate_kbps(kbps);
                                        ctx.registry.update(&session_id, |i| i.stats.bitrate_kbps = kbps);
                                    }
                                }
                            }
                            Some(Msg::Goodbye(g)) => { info!(reason = %g.reason, "client said goodbye"); break "client goodbye"; }
                            Some(Msg::Error(e)) => { warn!(code = e.code, msg = %e.message, "client error"); break "client error"; }
                            _ => {}
                        }
                    }
                }
                f = frames.recv() => {
                    let Some(f) = f else { return Err(Fatal::new(ErrorCode::EncoderFailed, "encoder stopped")); };
                    if waiting_for_key && !f.keyframe {
                        continue;
                    }
                    waiting_for_key = false;
                    let len = f.data.len() as u64;
                    let env = Envelope::new(Msg::VideoFrame(pb::VideoFrame {
                        sequence,
                        capture_time_us: f.capture_time_us,
                        encoded_time_us: f.encoded_time_us,
                        keyframe: f.keyframe,
                        data: f.data,
                        generation: 0,
                    }));
                    match self.vid.try_send(env) {
                        Ok(()) => {
                            sequence += 1;
                            ctx.registry.update(&session_id, |i| { i.stats.frames_sent += 1; i.stats.bytes_sent += len; });
                        }
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            // Network can't keep up: drop until the next keyframe.
                            waiting_for_key = true;
                            encoder.force_keyframe();
                            ctx.registry.update(&session_id, |i| i.stats.frames_dropped += 1);
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => break "connection closed",
                    }
                }
                e = errors.recv() => {
                    if let Some(e) = e {
                        return Err(Fatal::new(ErrorCode::EncoderFailed, e));
                    }
                }
                _ = ping.tick() => {
                    if last_rx.elapsed() > Duration::from_secs(10) {
                        return Err(Fatal::new(ErrorCode::Internal, "client timed out"));
                    }
                    ping_id += 1;
                    let _ = self.ctl.try_send(Envelope::new(Msg::Ping(pb::Ping { id: ping_id, sender_time_us: slate_media::now_us() })));
                }
                _ = placement_tick.tick() => {
                    stats_ticks += 1;
                    if stats_ticks % 5 == 0 {
                        if let Some(i) = ctx.registry.list().into_iter().find(|i| i.session_id == session_id) {
                            let secs = 10.0;
                            let fps = (i.stats.frames_sent - last_frames) as f64 / secs;
                            let kbps = (i.stats.bytes_sent - last_bytes) as f64 * 8.0 / secs / 1000.0;
                            last_frames = i.stats.frames_sent;
                            last_bytes = i.stats.bytes_sent;
                            info!(
                                %device_name,
                                fps = format!("{fps:.1}"),
                                kbps = format!("{kbps:.0}"),
                                target_kbps = i.stats.bitrate_kbps,
                                rtt_ms = format!("{:.1}", f64::from(i.stats.rtt_us) / 1000.0),
                                latency_ms = format!("{:.1}", f64::from(i.stats.latency_us) / 1000.0),
                                dropped = i.stats.frames_dropped,
                                "stream stats"
                            );
                        }
                    }
                    if let Ok(Some(info)) = ctx.backends.outputs.query(&output_name).await {
                        let p = info.placement;
                        geometry = Some(info.geometry);
                        if let Ok(mut s) = ctx.store.lock() {
                            if s.set_placement(&device_id, p).unwrap_or(false) {
                                info!(%output_name, x = p.x, y = p.y, scale = p.scale, "remembered new placement");
                            }
                        }
                        match (expected_mode, info.mode) {
                            (None, m) => expected_mode = m,
                            (Some(old), Some(m)) if mode_changed(old, m) => {
                                // KWin already switched the output, which stalls the capture
                                // stream; rebuild the stream at the new mode.
                                let new = crate::stream_settings::settings_for_mode(
                                    m.width, m.height, m.refresh_hz,
                                    plan.native_width, plan.native_height, plan.panel_fps,
                                );
                                info!(%output_name, w = m.width, h = m.height, hz = m.refresh_hz,
                                      resolution = %new.resolution, fps = new.fps,
                                      "display mode changed in Display & Monitor settings");
                                expected_mode = Some(m);
                                let stored = ctx.store.lock().ok().and_then(|mut s| s.set_stream_settings(&device_id, new).ok());
                                if stored == Some(true) {
                                    reconnect.store(true, std::sync::atomic::Ordering::SeqCst);
                                    cancel.cancel();
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        };
        drop(encoder);
        drop(output);
        Ok(reason)
    }

    fn cleanup(&mut self) {
        if let Some(slot) = self.slot.take() {
            let lo = slot * TOUCH_IDS_PER_SLOT;
            self.ctx
                .backends
                .touch
                .release_where(&move |id| (lo..lo + TOUCH_IDS_PER_SLOT).contains(&id));
        }
        if let Some(id) = self.session_id.take() {
            self.ctx.registry.remove(&id);
        }
    }
}

/// Whether the output now runs a different mode than the stream was set up for.
fn mode_changed(old: crate::backend::CurrentMode, new: crate::backend::CurrentMode) -> bool {
    (old.width, old.height) != (new.width, new.height)
        || (old.refresh_hz - new.refresh_hz).abs() > 0.5
}

fn pong(p: &pb::Ping) -> Envelope {
    Envelope::new(Msg::Pong(pb::Pong {
        id: p.id,
        ping_sender_time_us: p.sender_time_us,
        responder_time_us: slate_media::now_us(),
    }))
}

/// AIMD bitrate control from congestion signals.
#[derive(Debug, Clone)]
pub struct BitrateController {
    pub current: u32,
    pub max: u32,
    pub min: u32,
    calm_intervals: u32,
    min_rtt_us: u32,
}

impl BitrateController {
    pub fn new(start: u32) -> Self {
        Self {
            current: start,
            max: start,
            min: 2_000.min(start),
            calm_intervals: 0,
            min_rtt_us: u32::MAX,
        }
    }

    /// Feeds one stats interval; returns a new bitrate when it should change.
    ///
    /// Only *network* signals are used: `dropped` = frames the server had to drop because the
    /// connection could not keep up, `rtt_us` = smoothed round-trip time. Display latency is not a
    /// congestion signal (it also contains decoder buffering).
    pub fn update(&mut self, dropped: u64, rtt_us: u32) -> Option<u32> {
        if rtt_us > 0 {
            self.min_rtt_us = self.min_rtt_us.min(rtt_us);
        }
        let queueing = rtt_us > 0
            && self.min_rtt_us != u32::MAX
            && rtt_us > 80_000
            && rtt_us > self.min_rtt_us.saturating_mul(4);
        if dropped > 0 || queueing {
            self.calm_intervals = 0;
            let next = ((f64::from(self.current) * 0.7) as u32).max(self.min);
            return (next != self.current).then(|| {
                self.current = next;
                next
            });
        }
        self.calm_intervals += 1;
        if self.calm_intervals >= 3 && self.current < self.max {
            self.calm_intervals = 0;
            let next = ((f64::from(self.current) * 1.25) as u32).min(self.max);
            self.current = next;
            return Some(next);
        }
        None
    }
}

/// Converts a protocol touch event into EIS actions in global coordinates.
pub fn touch_actions(t: &pb::TouchEvent, slot: u32, region: Geometry) -> Vec<TouchAction> {
    let rect = Rect::new(region.x, region.y, region.width, region.height);
    t.points
        .iter()
        .filter_map(|p| {
            let id = slot * TOUCH_IDS_PER_SLOT + (p.id % TOUCH_IDS_PER_SLOT);
            let (x, y) = rect.map_normalized(p.x, p.y);
            match pb::TouchPhase::try_from(p.phase).ok()? {
                pb::TouchPhase::Down => Some(TouchAction::Down { id, x, y }),
                pb::TouchPhase::Move => Some(TouchAction::Motion { id, x, y }),
                pb::TouchPhase::Up => Some(TouchAction::Up { id }),
                pb::TouchPhase::Cancel => Some(TouchAction::Cancel { id }),
                pb::TouchPhase::Unspecified => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(w: u32, h: u32, decs: Vec<pb::DecoderInfo>) -> pb::ClientHello {
        pb::ClientHello {
            protocol_version: PROTOCOL_VERSION,
            device_id: "0123456789abcdef".into(),
            display: Some(pb::DisplayInfo {
                width_px: w,
                height_px: h,
                refresh_hz: 60.0,
                density_dpi: 320,
                ..Default::default()
            }),
            decoders: decs,
            ..Default::default()
        }
    }

    fn dec(c: pb::Codec, mw: u32, mh: u32) -> pb::DecoderInfo {
        pb::DecoderInfo {
            codec: c as i32,
            max_width: mw,
            max_height: mh,
            max_fps: 60,
            hardware: true,
            name: "x".into(),
        }
    }

    #[test]
    fn plan_is_landscape_and_prefers_h264() {
        let both = || {
            vec![
                dec(pb::Codec::Hevc, 4096, 2304),
                dec(pb::Codec::H264, 4096, 2304),
            ]
        };
        // Forced H.264.
        let p = plan_stream(&hello(1200, 1920, both()), Some(false)).unwrap();
        assert_eq!(
            (p.codec, p.width, p.height, p.fps),
            (Codec::H264, 1920, 1200, 60)
        );
        assert_eq!(
            (p.native_width, p.native_height, p.panel_fps),
            (1920, 1200, 60)
        );
        // Forced HEVC.
        assert_eq!(
            plan_stream(&hello(1200, 1920, both()), Some(true))
                .unwrap()
                .codec,
            Codec::Hevc
        );
        // Auto follows the client's order.
        assert_eq!(
            plan_stream(&hello(1200, 1920, both()), None).unwrap().codec,
            Codec::Hevc
        );
        let rev = vec![
            dec(pb::Codec::H264, 4096, 2304),
            dec(pb::Codec::Hevc, 4096, 2304),
        ];
        assert_eq!(
            plan_stream(&hello(1200, 1920, rev), None).unwrap().codec,
            Codec::H264
        );
        // Forcing a codec the client lacks falls back to what it has.
        assert_eq!(
            plan_stream(
                &hello(1200, 1920, vec![dec(pb::Codec::H264, 0, 0)]),
                Some(true)
            )
            .unwrap()
            .codec,
            Codec::H264
        );
    }

    #[test]
    fn plan_respects_decoder_limits() {
        let p = plan_stream(
            &hello(2560, 1600, vec![dec(pb::Codec::H264, 1920, 1088)]),
            None,
        )
        .unwrap();
        assert!(p.width <= 1920 && p.height <= 1088, "{p:?}");
        assert_eq!(p.width % 2, 0);
        assert_eq!(p.height % 2, 0);
    }

    #[test]
    fn plan_rejects_bad_input() {
        assert!(plan_stream(&hello(0, 100, vec![dec(pb::Codec::H264, 0, 0)]), None).is_err());
        assert_eq!(
            plan_stream(&hello(100, 100, vec![]), None).unwrap_err().0,
            ErrorCode::NoCommonCodec
        );
    }

    #[test]
    fn output_names() {
        assert_eq!(
            output_base_name("3F2A91C0-1111-2222"),
            "PlasmaSlate-3f2a91c0"
        );
        assert_eq!(output_base_name("---"), "PlasmaSlate-tablet");
    }

    mod e2e {
        use super::*;
        use crate::backend::mock::*;
        use crate::backend::OutputBackend;
        use slate_protocol::framing::{read_frame, write_frame};

        fn ctx(
            dir: &std::path::Path,
            approve: bool,
        ) -> (Arc<SessionContext>, Arc<MockOutputs>, Arc<MockTouch>) {
            let outputs = Arc::new(MockOutputs::default());
            let touch = Arc::new(MockTouch::default());
            let ctx = Arc::new(SessionContext {
                backends: Backends {
                    outputs: outputs.clone(),
                    touch: touch.clone(),
                    encoders: Arc::new(MockEncoders),
                    approver: Arc::new(FixedApprover(approve)),
                },
                store: Arc::new(Mutex::new(Store::load(dir.join("devices.json")).unwrap())),
                config: Arc::new(Config::default()),
                server_name: "test-pc".into(),
                fingerprint_short: "ABCD-1234".into(),
                registry: Arc::new(Registry::default()),
            });
            (ctx, outputs, touch)
        }

        async fn next_non_ping<S: tokio::io::AsyncRead + Unpin>(s: &mut S) -> Envelope {
            loop {
                let e = tokio::time::timeout(Duration::from_secs(5), read_frame(s))
                    .await
                    .unwrap()
                    .unwrap();
                if !matches!(e.msg, Some(Msg::Ping(_))) {
                    return e;
                }
            }
        }

        #[tokio::test]
        async fn pair_stream_touch_then_reconnect_with_token() {
            let dir = tempfile::tempdir().unwrap();
            let (ctx, outputs, touch) = ctx(dir.path(), true);
            let shutdown = CancellationToken::new();

            // --- first connection: pairing
            let (mut client, server) = tokio::io::duplex(1 << 20);
            let task = tokio::spawn(run(server, "test".into(), ctx.clone(), shutdown.clone()));
            write_frame(
                &mut client,
                &Envelope::new(Msg::ClientHello(hello(
                    1200,
                    1920,
                    vec![dec(pb::Codec::H264, 4096, 2304)],
                ))),
            )
            .await
            .unwrap();
            let e = next_non_ping(&mut client).await;
            assert!(matches!(e.msg, Some(Msg::PairingPending(_))), "{e:?}");
            let Some(Msg::ServerHello(sh)) = next_non_ping(&mut client).await.msg else {
                panic!("no server hello")
            };
            assert_eq!(sh.output_name, "Virtual-PlasmaSlate-01234567");
            assert!(!sh.issued_auth_token.is_empty());
            let stream = sh.stream.unwrap();
            assert_eq!((stream.width, stream.height), (1920, 1200));
            write_frame(&mut client, &Envelope::new(Msg::Ready(pb::Ready {})))
                .await
                .unwrap();

            let Some(Msg::VideoFrame(f)) = next_non_ping(&mut client).await.msg else {
                panic!("no video")
            };
            assert!(f.keyframe);
            assert_eq!(f.sequence, 0);

            write_frame(
                &mut client,
                &Envelope::new(Msg::TouchEvent(pb::TouchEvent {
                    time_us: 1,
                    points: vec![pb::TouchPoint {
                        id: 3,
                        phase: pb::TouchPhase::Down as i32,
                        x: 0.5,
                        y: 0.5,
                    }],
                })),
            )
            .await
            .unwrap();
            for _ in 0..5 {
                let _ = next_non_ping(&mut client).await;
            }
            {
                let acts = touch.actions.lock().unwrap();
                // Mock output at 3440,0, 1920x1200 at scale 1.25 (224 dpi → 1.5? default_scale(320)=2.25)
                let TouchAction::Down { id, x, y } = acts[0] else {
                    panic!("{acts:?}")
                };
                assert_eq!(id, 3);
                let crate::backend::OutputInfo {
                    placement: p,
                    geometry: g,
                    ..
                } = futures_lite_block(outputs.query("Virtual-PlasmaSlate-01234567"));
                assert_eq!(
                    (x, y),
                    (g.x + g.width / 2.0, g.y + g.height / 2.0),
                    "placement {p:?}"
                );
            }
            assert_eq!(ctx.registry.len(), 1);
            write_frame(
                &mut client,
                &Envelope::new(Msg::Goodbye(pb::Goodbye {
                    reason: "bye".into(),
                    ..Default::default()
                })),
            )
            .await
            .unwrap();
            task.await.unwrap();
            assert_eq!(ctx.registry.len(), 0);

            // --- second connection: token accepted, no pairing prompt
            let (mut client, server) = tokio::io::duplex(1 << 20);
            let task = tokio::spawn(run(server, "test".into(), ctx.clone(), shutdown.clone()));
            let mut h = hello(1200, 1920, vec![dec(pb::Codec::H264, 4096, 2304)]);
            h.auth_token = sh.issued_auth_token.clone();
            write_frame(&mut client, &Envelope::new(Msg::ClientHello(h)))
                .await
                .unwrap();
            let Some(Msg::ServerHello(sh2)) = next_non_ping(&mut client).await.msg else {
                panic!("expected direct ServerHello")
            };
            assert!(sh2.issued_auth_token.is_empty());
            write_frame(&mut client, &Envelope::new(Msg::Ready(pb::Ready {})))
                .await
                .unwrap();
            assert!(matches!(
                next_non_ping(&mut client).await.msg,
                Some(Msg::VideoFrame(_))
            ));

            // Disconnect from the PC side.
            assert!(ctx.registry.cancel(&sh2.session_id));
            loop {
                match tokio::time::timeout(Duration::from_secs(5), read_frame(&mut client))
                    .await
                    .unwrap()
                {
                    Ok(Envelope {
                        msg: Some(Msg::Goodbye(_)),
                    }) => break,
                    Ok(_) => continue,
                    Err(e) => panic!("{e}"),
                }
            }
            task.await.unwrap();
            assert_eq!(outputs.created.lock().unwrap().len(), 2);
        }

        #[tokio::test]
        async fn denied_pairing_gets_error() {
            let dir = tempfile::tempdir().unwrap();
            let (ctx, outputs, _) = ctx(dir.path(), false);
            let (mut client, server) = tokio::io::duplex(1 << 16);
            let task = tokio::spawn(run(server, "test".into(), ctx, CancellationToken::new()));
            write_frame(
                &mut client,
                &Envelope::new(Msg::ClientHello(hello(
                    1200,
                    1920,
                    vec![dec(pb::Codec::H264, 0, 0)],
                ))),
            )
            .await
            .unwrap();
            assert!(matches!(
                next_non_ping(&mut client).await.msg,
                Some(Msg::PairingPending(_))
            ));
            let Some(Msg::Error(e)) = next_non_ping(&mut client).await.msg else {
                panic!()
            };
            assert_eq!(e.code, ErrorCode::PairingDenied as i32);
            task.await.unwrap();
            assert!(outputs.created.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn stored_settings_shrink_stream_and_reconnect_on_change() {
            let dir = tempfile::tempdir().unwrap();
            let (ctx, outputs, _) = ctx(dir.path(), true);
            // Pair first so the device exists in the store.
            let tok = ctx
                .store
                .lock()
                .unwrap()
                .pair("0123456789abcdef", "Tablet")
                .unwrap();
            ctx.store
                .lock()
                .unwrap()
                .set_stream_settings(
                    "0123456789abcdef",
                    crate::stream_settings::StreamSettings {
                        resolution: "1280x800".into(),
                        fps: 30,
                    },
                )
                .unwrap();
            let (mut client, server) = tokio::io::duplex(1 << 20);
            let task = tokio::spawn(run(
                server,
                "test".into(),
                ctx.clone(),
                CancellationToken::new(),
            ));
            let mut h = hello(1200, 1920, vec![dec(pb::Codec::H264, 4096, 2304)]);
            h.auth_token = tok;
            write_frame(&mut client, &Envelope::new(Msg::ClientHello(h)))
                .await
                .unwrap();
            let Some(Msg::ServerHello(sh)) = next_non_ping(&mut client).await.msg else {
                panic!("expected ServerHello")
            };
            let st = sh.stream.unwrap();
            assert_eq!((st.width, st.height, st.max_fps), (1280, 800, 30));
            // The virtual output is created at the reduced size.
            let (_, w, h) = outputs.placements.lock().unwrap()[&sh.output_name];
            assert_eq!((w, h), (1280, 800));
            write_frame(&mut client, &Envelope::new(Msg::Ready(pb::Ready {})))
                .await
                .unwrap();
            assert!(matches!(
                next_non_ping(&mut client).await.msg,
                Some(Msg::VideoFrame(_))
            ));
            let info = ctx.registry.list().pop().unwrap();
            assert_eq!(
                (info.width, info.fps, info.native_width, info.panel_fps),
                (1280, 30, 1920, 60)
            );

            // Settings change while connected: the tablet is told to reconnect.
            assert_eq!(ctx.registry.reconnect_device("0123456789abcdef"), 1);
            loop {
                match tokio::time::timeout(Duration::from_secs(5), read_frame(&mut client))
                    .await
                    .unwrap()
                {
                    Ok(Envelope {
                        msg: Some(Msg::Goodbye(g)),
                    }) => {
                        assert!(g.reconnect);
                        break;
                    }
                    Ok(_) => continue,
                    Err(e) => panic!("{e}"),
                }
            }
            task.await.unwrap();
        }

        #[tokio::test]
        async fn display_settings_mode_change_is_stored_and_reconnects() {
            let dir = tempfile::tempdir().unwrap();
            let (ctx, outputs, _) = ctx(dir.path(), true);
            let tok = ctx
                .store
                .lock()
                .unwrap()
                .pair("0123456789abcdef", "Tablet")
                .unwrap();
            let (mut client, server) = tokio::io::duplex(1 << 20);
            let task = tokio::spawn(run(
                server,
                "test".into(),
                ctx.clone(),
                CancellationToken::new(),
            ));
            let mut h = hello(1200, 1920, vec![dec(pb::Codec::H264, 4096, 2304)]);
            h.auth_token = tok;
            write_frame(&mut client, &Envelope::new(Msg::ClientHello(h)))
                .await
                .unwrap();
            let Some(Msg::ServerHello(sh)) = next_non_ping(&mut client).await.msg else {
                panic!("expected ServerHello")
            };
            // The preset sizes and refresh rates are offered to Display & Monitor settings.
            let offered = outputs.offered.lock().unwrap()[&sh.output_name].clone();
            assert!(offered.contains(&(1920, 1200, 60)));
            assert!(offered.contains(&(1440, 900, 30)));
            write_frame(&mut client, &Envelope::new(Msg::Ready(pb::Ready {})))
                .await
                .unwrap();
            assert!(matches!(
                next_non_ping(&mut client).await.msg,
                Some(Msg::VideoFrame(_))
            ));

            // The user picks 1440×900 @ 30 Hz in Display & Monitor settings.
            outputs.modes.lock().unwrap().insert(
                sh.output_name.clone(),
                crate::backend::CurrentMode {
                    width: 1440,
                    height: 900,
                    refresh_hz: 30.0,
                },
            );
            loop {
                match tokio::time::timeout(Duration::from_secs(6), read_frame(&mut client))
                    .await
                    .unwrap()
                {
                    Ok(Envelope {
                        msg: Some(Msg::Goodbye(g)),
                    }) => {
                        assert!(g.reconnect);
                        break;
                    }
                    Ok(_) => continue,
                    Err(e) => panic!("{e}"),
                }
            }
            task.await.unwrap();
            assert_eq!(
                ctx.store
                    .lock()
                    .unwrap()
                    .stream_settings("0123456789abcdef"),
                Some(crate::stream_settings::StreamSettings {
                    resolution: "75%".into(),
                    fps: 30
                })
            );
        }

        #[tokio::test]
        async fn wrong_protocol_version_is_rejected() {
            let dir = tempfile::tempdir().unwrap();
            let (ctx, _, _) = ctx(dir.path(), true);
            let (mut client, server) = tokio::io::duplex(1 << 16);
            let task = tokio::spawn(run(server, "test".into(), ctx, CancellationToken::new()));
            let mut h = hello(1200, 1920, vec![dec(pb::Codec::H264, 0, 0)]);
            h.protocol_version = 99;
            write_frame(&mut client, &Envelope::new(Msg::ClientHello(h)))
                .await
                .unwrap();
            let Some(Msg::Error(e)) = next_non_ping(&mut client).await.msg else {
                panic!()
            };
            assert_eq!(e.code, ErrorCode::ProtocolVersion as i32);
            task.await.unwrap();
        }

        fn futures_lite_block<
            F: std::future::Future<Output = anyhow::Result<Option<crate::backend::OutputInfo>>>,
        >(
            f: F,
        ) -> crate::backend::OutputInfo {
            // MockOutputs::query never awaits, so polling once is enough.
            let waker = std::task::Waker::noop();
            let mut cx = std::task::Context::from_waker(waker);
            let mut f = std::pin::pin!(f);
            match f.as_mut().poll(&mut cx) {
                std::task::Poll::Ready(r) => r.unwrap().unwrap(),
                std::task::Poll::Pending => panic!("mock query pending"),
            }
        }
    }

    #[test]
    fn bitrate_controller_backs_off_and_recovers() {
        let mut c = BitrateController::new(20_000);
        assert_eq!(c.update(0, 18_000), None);
        // Send-queue drops: multiplicative decrease.
        assert_eq!(c.update(3, 18_000), Some(14_000));
        // RTT far above its floor (queueing): decrease.
        assert_eq!(c.update(0, 200_000), Some(9_800));
        // High display latency is not passed in; calm intervals recover.
        assert_eq!(c.update(0, 19_000), None);
        assert_eq!(c.update(0, 19_000), None);
        assert_eq!(c.update(0, 19_000), Some(12_250));
        // A slow but stable link (rtt below 80 ms) is not congestion.
        let mut wan = BitrateController::new(10_000);
        assert_eq!(wan.update(0, 60_000), None);
        let mut low = BitrateController::new(2_500);
        for _ in 0..10 {
            low.update(5, 0);
        }
        assert_eq!(low.current, 2_000);
    }

    #[test]
    fn touch_mapping_namespaces_ids() {
        let t = pb::TouchEvent {
            time_us: 0,
            points: vec![
                pb::TouchPoint {
                    id: 0,
                    phase: pb::TouchPhase::Down as i32,
                    x: 0.5,
                    y: 0.5,
                },
                pb::TouchPoint {
                    id: 1,
                    phase: pb::TouchPhase::Up as i32,
                    x: 0.0,
                    y: 0.0,
                },
            ],
        };
        let a = touch_actions(
            &t,
            2,
            Geometry {
                x: 100.0,
                y: 200.0,
                width: 1000.0,
                height: 500.0,
            },
        );
        assert_eq!(
            a[0],
            TouchAction::Down {
                id: 128,
                x: 600.0,
                y: 450.0
            }
        );
        assert_eq!(a[1], TouchAction::Up { id: 129 });
    }
}
