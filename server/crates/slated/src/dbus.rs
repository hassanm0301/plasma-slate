//! Session-bus API used by the tray app.
//!
//! Service `io.github.hassanm0301.PlasmaSlate`, object `/io/github/hassanm0301/PlasmaSlate`,
//! interface `io.github.hassanm0301.PlasmaSlate1`.

use std::sync::{Arc, Mutex};

use tracing::{info, warn};
use zbus::object_server::SignalEmitter;

use crate::session::Registry;
use crate::store::Store;

pub const SERVICE: &str = "io.github.hassanm0301.PlasmaSlate";
pub const PATH: &str = "/io/github/hassanm0301/PlasmaSlate";
pub const INTERFACE: &str = "io.github.hassanm0301.PlasmaSlate1";

pub struct DaemonIface {
    pub registry: Arc<Registry>,
    pub store: Arc<Mutex<Store>>,
    pub server_name: String,
    pub fingerprint: String,
    pub port: u16,
    /// Daemon-wide default stream settings (from slated.toml).
    pub defaults: crate::stream_settings::StreamSettings,
}

/// (session_id, device_id, device_name, output_name, peer, link, width, height, encoder,
///  started_unix, frames_sent, bytes_sent, frames_dropped, rtt_us, latency_us, bitrate_kbps)
pub type SessionTuple = (
    String,
    String,
    String,
    String,
    String,
    String,
    u32,
    u32,
    String,
    u64,
    u64,
    u64,
    u64,
    u32,
    u32,
    u32,
);

#[zbus::interface(name = "io.github.hassanm0301.PlasmaSlate1")]
impl DaemonIface {
    /// Connected tablets.
    fn sessions(&self) -> Vec<SessionTuple> {
        self.registry
            .list()
            .into_iter()
            .map(|s| {
                (
                    s.session_id,
                    s.device_id,
                    s.device_name,
                    s.output_name,
                    s.peer,
                    s.link,
                    s.width,
                    s.height,
                    s.encoder,
                    s.started_unix,
                    s.stats.frames_sent,
                    s.stats.bytes_sent,
                    s.stats.frames_dropped,
                    s.stats.rtt_us,
                    s.stats.latency_us,
                    s.stats.bitrate_kbps,
                )
            })
            .collect()
    }

    /// Same as `Sessions`, as a JSON array of objects (easier for UI clients).
    fn sessions_json(&self) -> String {
        let v: Vec<serde_json::Value> = self
            .registry
            .list()
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "session_id": s.session_id,
                    "device_id": s.device_id,
                    "device_name": s.device_name,
                    "output_name": s.output_name,
                    "peer": s.peer,
                    "link": s.link,
                    "width": s.width,
                    "height": s.height,
                    "codec": s.codec,
                    "encoder": s.encoder,
                    "started_unix": s.started_unix,
                    "frames_sent": s.stats.frames_sent,
                    "fillers_sent": s.stats.fillers_sent,
                    "bytes_sent": s.stats.bytes_sent,
                    "frames_dropped": s.stats.frames_dropped,
                    "rtt_us": s.stats.rtt_us,
                    "encode_us": s.stats.encode_us,
                    "client_decode_us": s.stats.client_decode_us,
                    "latency_us": s.stats.latency_us,
                    "bitrate_kbps": s.stats.bitrate_kbps,
                    "fps": s.fps,
                    "native_width": s.native_width,
                    "native_height": s.native_height,
                    "panel_fps": s.panel_fps,
                    "resolution_setting": s.settings.resolution,
                    "fps_setting": s.settings.fps,
                    // The same choices Display & Monitor settings offers for this screen.
                    "resolution_presets": crate::stream_settings::resolution_presets(s.native_width, s.native_height)
                        .into_iter()
                        .map(|(r, w, h)| serde_json::json!({"value": r.to_string(), "width": w, "height": h}))
                        .collect::<Vec<_>>(),
                    "refresh_presets": crate::stream_settings::refresh_presets(s.panel_fps),
                })
            })
            .collect();
        serde_json::Value::Array(v).to_string()
    }

    /// Paired devices as JSON: `[{"device_id", "name", "last_seen"}]`.
    fn paired_devices_json(&self) -> String {
        let v: Vec<serde_json::Value> = self
            .store
            .lock()
            .map(|s| s.devices().map(|(id, d)| serde_json::json!({"device_id": id, "name": d.name, "last_seen": d.last_seen})).collect())
            .unwrap_or_default();
        serde_json::Value::Array(v).to_string()
    }

    /// Paired devices: (device_id, name, last_seen_unix).
    fn paired_devices(&self) -> Vec<(String, String, u64)> {
        self.store
            .lock()
            .map(|s| {
                s.devices()
                    .map(|(id, d)| (id.clone(), d.name.clone(), d.last_seen))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Stream settings of a device: (resolution, fps). Devices without their own settings
    /// report the daemon defaults.
    fn get_stream_settings(&self, device_id: String) -> (String, u32) {
        let s = self
            .store
            .lock()
            .ok()
            .and_then(|s| s.stream_settings(&device_id))
            .unwrap_or_else(|| self.defaults.clone());
        (s.resolution, s.fps)
    }

    /// Sets a device's resolution (`native`, `NN%`, `WIDTHxHEIGHT`) and refresh rate (0 = auto).
    /// A connected tablet reconnects immediately with the new settings. Returns an error
    /// message, or an empty string on success.
    fn set_stream_settings(&self, device_id: String, resolution: String, fps: u32) -> String {
        let s = match (crate::stream_settings::StreamSettings { resolution, fps }).validated() {
            Ok(s) => s,
            Err(e) => return e,
        };
        let stored = self
            .store
            .lock()
            .map_err(|_| "store unavailable".to_string())
            .and_then(|mut st| {
                st.set_stream_settings(&device_id, s.clone())
                    .map_err(|e| e.to_string())
            });
        match stored {
            Ok(true) => {
                info!(%device_id, resolution = %s.resolution, fps = s.fps, "stream settings changed");
                self.registry.reconnect_device(&device_id);
                String::new()
            }
            Ok(false) => format!("unknown device {device_id}"),
            Err(e) => e,
        }
    }

    /// Disconnects a session.
    fn disconnect(&self, session_id: String) -> bool {
        info!(%session_id, "disconnect requested over D-Bus");
        self.registry.cancel(&session_id)
    }

    /// Forgets a device (it must pair again) and disconnects it.
    fn unpair(&self, device_id: String) -> bool {
        info!(%device_id, "unpair requested over D-Bus");
        self.registry.cancel_device(&device_id);
        self.store
            .lock()
            .map(|mut s| s.unpair(&device_id).unwrap_or(false))
            .unwrap_or(false)
    }

    #[zbus(property)]
    fn server_name(&self) -> String {
        self.server_name.clone()
    }

    #[zbus(property)]
    fn fingerprint(&self) -> String {
        self.fingerprint.clone()
    }

    #[zbus(property)]
    fn port(&self) -> u16 {
        self.port
    }

    #[zbus(property)]
    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    /// Emitted whenever a session starts, ends or changes.
    #[zbus(signal)]
    async fn sessions_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Registers the service and forwards registry changes as signals. Returns the connection,
/// which must be kept alive.
pub async fn start(iface: DaemonIface) -> anyhow::Result<zbus::Connection> {
    let registry = iface.registry.clone();
    let conn = zbus::connection::Builder::session()?
        .name(SERVICE)?
        .serve_at(PATH, iface)?
        .build()
        .await?;
    let iface_ref = conn
        .object_server()
        .interface::<_, DaemonIface>(PATH)
        .await?;
    let mut rx = registry.changed.subscribe();
    tokio::spawn(async move {
        while let Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) =
            rx.recv().await
        {
            if let Err(e) = DaemonIface::sessions_changed(iface_ref.signal_emitter()).await {
                warn!("emit SessionsChanged: {e}");
            }
        }
    });
    info!(service = SERVICE, "D-Bus API ready");
    Ok(conn)
}
