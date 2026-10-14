//! Abstractions over the platform pieces a session needs, so session logic can be tested
//! without KWin, PipeWire or a GPU.
//!
//! Real implementations: [`kwin`] (KWin + GStreamer). Test doubles: [`mock`].

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

pub use slate_media::{Codec, EncodedFrame, EncoderConfig};

/// Placement of an output in the global layout.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub scale: f64,
}

/// Geometry of an output in global logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A created virtual output.
#[async_trait]
pub trait OutputHandle: Send + Sync {
    /// KWin output name (e.g. `Virtual-PlasmaSlate-1a2b3c4d`).
    fn output_name(&self) -> &str;
    /// PipeWire target for capture.
    fn pipewire_target(&self) -> String;
    /// Resolves when the compositor removes the output.
    async fn closed(&mut self);
}

/// Creates and configures outputs.
#[async_trait]
pub trait OutputBackend: Send + Sync {
    async fn create(&self, name: &str, description: &str, width: u32, height: u32) -> anyhow::Result<Box<dyn OutputHandle>>;
    /// Selects the best mode for `width`×`height` and applies the placement. When `placement`
    /// is `None` a default is chosen from `dpi`. Returns the placement in effect.
    async fn configure(&self, output: &str, width: u32, height: u32, max_hz: f64, placement: Option<Placement>, dpi: f64)
        -> anyhow::Result<Placement>;
    /// Current placement and geometry, if the output exists.
    async fn query(&self, output: &str) -> anyhow::Result<Option<(Placement, Geometry)>>;
}

/// Touch injection.
pub trait TouchBackend: Send + Sync {
    /// Region of the output as known to the input system (preferred over [`OutputBackend::query`]).
    fn region(&self, output: &str) -> Option<Geometry>;
    fn send(&self, actions: &[slate_kwin::eis::TouchAction]) -> anyhow::Result<()>;
    fn release_where(&self, pred: &dyn Fn(u32) -> bool);
}

/// A running encoder.
pub trait EncoderHandle: Send {
    fn force_keyframe(&self);
    /// Used by adaptive bitrate (M5).
    #[allow(dead_code)]
    fn set_bitrate_kbps(&self, kbps: u32);
    fn variant(&self) -> &'static str;
}

/// Output of [`EncoderFactory::start`].
pub struct StartedEncoder {
    pub handle: Box<dyn EncoderHandle>,
    pub frames: mpsc::Receiver<EncodedFrame>,
    pub errors: mpsc::UnboundedReceiver<String>,
}

impl std::fmt::Debug for StartedEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartedEncoder").field("variant", &self.handle.variant()).finish_non_exhaustive()
    }
}

/// Starts encoders.
pub trait EncoderFactory: Send + Sync {
    fn start(&self, cfg: &EncoderConfig) -> anyhow::Result<StartedEncoder>;
}

/// Asks the user whether a new device may connect.
#[async_trait]
pub trait Approver: Send + Sync {
    async fn approve(&self, device_name: &str, device_id: &str, server_fingerprint: &str) -> bool;
}

/// All backends a session needs.
#[derive(Clone)]
pub struct Backends {
    pub outputs: Arc<dyn OutputBackend>,
    pub touch: Arc<dyn TouchBackend>,
    pub encoders: Arc<dyn EncoderFactory>,
    pub approver: Arc<dyn Approver>,
}

impl std::fmt::Debug for Backends {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backends").finish_non_exhaustive()
    }
}

/// Real implementations on top of KWin and GStreamer.
pub mod kwin {
    use super::*;
    use slate_kwin::eis::TouchInjector;
    use slate_kwin::kscreen;
    use slate_kwin::virtual_output::{VirtualOutput, VirtualOutputManager};
    use std::time::Duration;
    use tokio::sync::Mutex;
    use tracing::{info, warn};

    pub struct KwinOutputs {
        manager: VirtualOutputManager,
    }

    impl KwinOutputs {
        pub fn connect() -> anyhow::Result<Self> {
            Ok(Self { manager: VirtualOutputManager::connect()? })
        }
    }

    struct Handle(VirtualOutput);

    #[async_trait]
    impl OutputHandle for Handle {
        fn output_name(&self) -> &str {
            &self.0.output_name
        }
        fn pipewire_target(&self) -> String {
            self.0.pipewire_target()
        }
        async fn closed(&mut self) {
            self.0.closed().await;
        }
    }

    #[async_trait]
    impl OutputBackend for KwinOutputs {
        async fn create(&self, name: &str, description: &str, width: u32, height: u32) -> anyhow::Result<Box<dyn OutputHandle>> {
            Ok(Box::new(Handle(self.manager.create(name, description, width, height).await?)))
        }

        async fn configure(
            &self,
            output: &str,
            width: u32,
            height: u32,
            max_hz: f64,
            placement: Option<Placement>,
            dpi: f64,
        ) -> anyhow::Result<Placement> {
            // The output shows up in kscreen shortly after KWin creates it.
            let mut state = None;
            for _ in 0..30 {
                if let Some(s) = kscreen::get(output).await? {
                    state = Some(s);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let state = state.ok_or_else(|| anyhow::anyhow!("output {output} did not appear in kscreen"))?;
            let mut change = kscreen::Change::default();
            if let Some(mode) = state.best_mode(width, height, max_hz) {
                if state.current_mode.as_ref().map(|m| &m.id) != Some(&mode.id) {
                    change.mode_id = Some(mode.id.clone());
                }
            } else {
                warn!(output, width, height, "no matching mode found");
            }
            let target = match placement {
                Some(p) => p,
                None => {
                    // First time we see this device: keep KWin's position, pick a scale from the panel density.
                    Placement { x: state.x, y: state.y, scale: kscreen::default_scale(dpi) }
                }
            };
            if (state.scale - target.scale).abs() > 0.001 {
                change.scale = Some(target.scale);
            }
            if (state.x, state.y) != (target.x, target.y) {
                change.position = Some((target.x, target.y));
            }
            if !change.is_empty() {
                info!(output, ?change, "configuring output");
                kscreen::apply(output, &change).await?;
            }
            Ok(target)
        }

        async fn query(&self, output: &str) -> anyhow::Result<Option<(Placement, Geometry)>> {
            Ok(kscreen::get(output).await?.and_then(|s| {
                let (w, h) = s.logical_size()?;
                Some((
                    Placement { x: s.x, y: s.y, scale: s.scale },
                    Geometry { x: f64::from(s.x), y: f64::from(s.y), width: w, height: h },
                ))
            }))
        }
    }

    /// EIS touch with lazy (re)connection.
    pub struct KwinTouch {
        injector: std::sync::Mutex<Option<TouchInjector>>,
        reconnecting: Mutex<()>,
    }

    impl KwinTouch {
        pub async fn connect() -> Self {
            let inj = match TouchInjector::connect().await {
                Ok(i) => Some(i),
                Err(e) => {
                    warn!("touch injection unavailable: {e}");
                    None
                }
            };
            Self { injector: std::sync::Mutex::new(inj), reconnecting: Mutex::new(()) }
        }

        fn injector(&self) -> Option<TouchInjector> {
            self.injector.lock().ok().and_then(|g| g.clone())
        }

        /// Reconnects if the EIS connection was lost.
        pub async fn ensure(&self) {
            if self.injector().is_some_and(|i| i.is_ready()) {
                return;
            }
            let _guard = self.reconnecting.lock().await;
            if self.injector().is_some_and(|i| i.is_ready()) {
                return;
            }
            match TouchInjector::connect().await {
                Ok(i) => {
                    if let Ok(mut g) = self.injector.lock() {
                        *g = Some(i);
                    }
                    info!("touch injection (re)connected");
                }
                Err(e) => warn!("touch reconnect failed: {e}"),
            }
        }
    }

    impl TouchBackend for KwinTouch {
        fn region(&self, output: &str) -> Option<Geometry> {
            self.injector()?.region(output).map(|r| Geometry { x: r.x, y: r.y, width: r.width, height: r.height })
        }
        fn send(&self, actions: &[slate_kwin::eis::TouchAction]) -> anyhow::Result<()> {
            let inj = self.injector().ok_or_else(|| anyhow::anyhow!("touch injection unavailable"))?;
            Ok(inj.send(actions)?)
        }
        fn release_where(&self, pred: &dyn Fn(u32) -> bool) {
            if let Some(i) = self.injector() {
                i.release_where(pred);
            }
        }
    }

    pub struct GstEncoders;

    struct GstHandle(slate_media::Encoder);

    impl EncoderHandle for GstHandle {
        fn force_keyframe(&self) {
            self.0.force_keyframe();
        }
        fn set_bitrate_kbps(&self, kbps: u32) {
            self.0.set_bitrate_kbps(kbps);
        }
        fn variant(&self) -> &'static str {
            self.0.variant()
        }
    }

    impl EncoderFactory for GstEncoders {
        fn start(&self, cfg: &EncoderConfig) -> anyhow::Result<StartedEncoder> {
            let (enc, outs) = slate_media::Encoder::start(cfg)?;
            Ok(StartedEncoder { handle: Box::new(GstHandle(enc)), frames: outs.frames, errors: outs.errors })
        }
    }
}

/// Test doubles.
#[cfg(test)]
pub mod mock {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Records outputs and placements; geometry is derived from placement and size.
    #[derive(Default)]
    pub struct MockOutputs {
        pub created: Mutex<Vec<String>>,
        pub placements: Mutex<std::collections::HashMap<String, (Placement, u32, u32)>>,
        pub closers: Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<()>>>,
    }

    struct MockHandle {
        name: String,
        closed: Option<tokio::sync::oneshot::Receiver<()>>,
    }

    #[async_trait]
    impl OutputHandle for MockHandle {
        fn output_name(&self) -> &str {
            &self.name
        }
        fn pipewire_target(&self) -> String {
            "mock".into()
        }
        async fn closed(&mut self) {
            match self.closed.as_mut() {
                Some(rx) => {
                    let _ = rx.await;
                    self.closed = None;
                }
                None => std::future::pending::<()>().await,
            }
        }
    }

    #[async_trait]
    impl OutputBackend for MockOutputs {
        async fn create(&self, name: &str, _d: &str, width: u32, height: u32) -> anyhow::Result<Box<dyn OutputHandle>> {
            let out = format!("Virtual-{name}");
            self.created.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.push(out.clone());
            self.placements
                .lock()
                .map_err(|_| anyhow::anyhow!("poisoned"))?
                .insert(out.clone(), (Placement { x: 3440, y: 0, scale: 1.0 }, width, height));
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.closers.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.insert(out.clone(), tx);
            Ok(Box::new(MockHandle { name: out, closed: Some(rx) }))
        }
        async fn configure(&self, output: &str, w: u32, h: u32, _hz: f64, placement: Option<Placement>, dpi: f64) -> anyhow::Result<Placement> {
            let p = placement.unwrap_or(Placement { x: 3440, y: 0, scale: slate_kwin::kscreen::default_scale(dpi) });
            self.placements.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.insert(output.to_string(), (p, w, h));
            Ok(p)
        }
        async fn query(&self, output: &str) -> anyhow::Result<Option<(Placement, Geometry)>> {
            Ok(self.placements.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.get(output).map(|(p, w, h)| {
                (*p, Geometry { x: f64::from(p.x), y: f64::from(p.y), width: f64::from(*w) / p.scale, height: f64::from(*h) / p.scale })
            }))
        }
    }

    /// Records touch actions.
    #[derive(Default)]
    pub struct MockTouch {
        pub actions: Mutex<Vec<slate_kwin::eis::TouchAction>>,
    }

    impl TouchBackend for MockTouch {
        fn region(&self, _output: &str) -> Option<Geometry> {
            None
        }
        fn send(&self, actions: &[slate_kwin::eis::TouchAction]) -> anyhow::Result<()> {
            self.actions.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.extend_from_slice(actions);
            Ok(())
        }
        fn release_where(&self, _pred: &dyn Fn(u32) -> bool) {}
    }

    /// Emits a synthetic keyframe followed by delta frames every 16 ms.
    pub struct MockEncoders;

    struct MockEnc(Arc<std::sync::atomic::AtomicBool>);

    impl EncoderHandle for MockEnc {
        fn force_keyframe(&self) {
            self.0.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        fn set_bitrate_kbps(&self, _kbps: u32) {}
        fn variant(&self) -> &'static str {
            "mock"
        }
    }

    impl EncoderFactory for MockEncoders {
        fn start(&self, _cfg: &EncoderConfig) -> anyhow::Result<StartedEncoder> {
            let (tx, frames) = mpsc::channel(8);
            let (_etx, errors) = mpsc::unbounded_channel();
            let key = Arc::new(std::sync::atomic::AtomicBool::new(true));
            let k2 = key.clone();
            tokio::spawn(async move {
                let _keep_errors_open = _etx;
                loop {
                    let keyframe = k2.swap(false, std::sync::atomic::Ordering::Relaxed);
                    let now = slate_media::now_us();
                    let nal = if keyframe { vec![0, 0, 0, 1, 0x67, 0, 0, 0, 1, 0x68, 0, 0, 0, 1, 0x65, 1] } else { vec![0, 0, 0, 1, 0x41, 2] };
                    if tx.send(EncodedFrame { data: nal, keyframe, capture_time_us: now, encoded_time_us: now }).await.is_err() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(16)).await;
                }
            });
            Ok(StartedEncoder { handle: Box::new(MockEnc(key)), frames, errors })
        }
    }

    /// Approves or denies everything.
    pub struct FixedApprover(pub bool);

    #[async_trait]
    impl Approver for FixedApprover {
        async fn approve(&self, _: &str, _: &str, _: &str) -> bool {
            self.0
        }
    }
}
