//! Virtual outputs through `zkde_screencast_unstable_v1`.
//!
//! A dedicated thread owns the Wayland event queue and dispatches events; requests are sent
//! from any thread (wayland-client proxies are thread-safe) followed by a flush.
//!
//! The same connection follows the `wl_output` globals, so the daemon knows which monitors
//! are connected (see [`VirtualOutputManager::monitors`]).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{debug, info, warn};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

use crate::{KwinError, Result};

#[allow(
    dead_code,
    non_camel_case_types,
    non_upper_case_globals,
    missing_docs,
    unused_imports
)]
#[allow(clippy::all, clippy::pedantic)]
mod protocol {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("../../../protocols/zkde-screencast-unstable-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("../../../protocols/zkde-screencast-unstable-v1.xml");
}

use protocol::zkde_screencast_stream_unstable_v1::{
    self as zstream, ZkdeScreencastStreamUnstableV1,
};
use protocol::zkde_screencast_unstable_v1::{self as zsc, ZkdeScreencastUnstableV1};

/// Prefix KWin puts in front of the requested name to form the output (connector) name.
pub const KWIN_VIRTUAL_PREFIX: &str = "Virtual-";

/// Lifecycle of a screencast stream as reported by KWin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamStatus {
    Pending,
    /// PipeWire object serial (preferred) and node id.
    Ready {
        serial: Option<u64>,
        node_id: u32,
    },
    Failed(String),
    Closed,
}

#[derive(Debug)]
struct StreamData {
    status: watch::Sender<StreamStatus>,
    serial: std::sync::Mutex<Option<u64>>,
}

#[derive(Debug)]
struct State {
    screencast: Option<(u32, u32)>,
    /// Bound `wl_output`s by global name, with their connector name once KWin has sent it.
    outputs: HashMap<u32, (wl_output::WlOutput, Option<String>)>,
    monitors: watch::Sender<Vec<String>>,
}

impl State {
    /// Publishes the connected monitors. Called after each batch of events, so an output that
    /// KWin replaces in one go (e.g. a monitor whose EDID changed) never looks like a gap.
    fn publish(&self) {
        let names = monitor_names(
            self.outputs
                .iter()
                .map(|(global, (_, name))| (*global, name.as_deref())),
        );
        self.monitors.send_if_modified(|cur| {
            let changed = *cur != names;
            if changed {
                debug!(?names, "monitors changed");
                *cur = names;
            }
            changed
        });
    }
}

/// Sorted names of the outputs that are not virtual. An output whose name is not known yet
/// (just announced, or `wl_output` older than v4) counts as a monitor under a placeholder
/// name: a monitor must never look absent.
fn monitor_names<'a>(outputs: impl Iterator<Item = (u32, Option<&'a str>)>) -> Vec<String> {
    let mut names: Vec<String> = outputs
        .filter_map(|(global, name)| match name {
            Some(n) if n.starts_with(KWIN_VIRTUAL_PREFIX) => None,
            Some(n) => Some(n.to_owned()),
            None => Some(format!("wl_output@{global}")),
        })
        .collect();
    names.sort();
    names
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "zkde_screencast_unstable_v1" => state.screencast = Some((name, version)),
                // v4 adds the connector name (`DP-1`, `Virtual-…`).
                "wl_output" => {
                    let output: wl_output::WlOutput = registry.bind(name, version.min(4), qh, name);
                    state.outputs.insert(name, (output, None));
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                if let Some((output, _)) = state.outputs.remove(&name) {
                    if output.version() >= 3 {
                        output.release();
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            if let Some((_, n)) = state.outputs.get_mut(global) {
                *n = Some(name);
            }
        }
    }
}

impl Dispatch<ZkdeScreencastUnstableV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZkdeScreencastUnstableV1,
        _: zsc::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZkdeScreencastStreamUnstableV1, Arc<StreamData>> for State {
    fn event(
        _: &mut Self,
        _: &ZkdeScreencastStreamUnstableV1,
        event: zstream::Event,
        data: &Arc<StreamData>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zstream::Event::Serial {
                object_serial_hi,
                object_serial_low,
            } => {
                let serial = (u64::from(object_serial_hi) << 32) | u64::from(object_serial_low);
                if let Ok(mut s) = data.serial.lock() {
                    *s = Some(serial);
                }
            }
            zstream::Event::Created { node } => {
                let serial = data.serial.lock().ok().and_then(|s| *s);
                data.status.send_replace(StreamStatus::Ready {
                    serial,
                    node_id: node,
                });
            }
            zstream::Event::Failed { error } => {
                data.status.send_replace(StreamStatus::Failed(error));
            }
            zstream::Event::Closed => {
                data.status.send_replace(StreamStatus::Closed);
            }
        }
    }
}

/// Connection to KWin able to create virtual outputs.
#[derive(Debug, Clone)]
pub struct VirtualOutputManager {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    conn: Connection,
    qh: QueueHandle<State>,
    screencast: ZkdeScreencastUnstableV1,
    version: u32,
    monitors: watch::Receiver<Vec<String>>,
}

impl VirtualOutputManager {
    /// Connects to the compositor named by `WAYLAND_DISPLAY` and starts the dispatch thread.
    pub fn connect() -> Result<Self> {
        let conn = Connection::connect_to_env().map_err(|e| KwinError::Wayland(e.to_string()))?;
        let mut queue: EventQueue<State> = conn.new_event_queue();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let (monitors_tx, monitors) = watch::channel(Vec::new());
        let mut state = State {
            screencast: None,
            outputs: HashMap::new(),
            monitors: monitors_tx,
        };
        // The second roundtrip delivers the names of the outputs bound during the first.
        for _ in 0..2 {
            queue
                .roundtrip(&mut state)
                .map_err(|e| KwinError::Wayland(e.to_string()))?;
        }
        state.publish();
        let (global, advertised) = state
            .screencast
            .ok_or(KwinError::Restricted("zkde_screencast_unstable_v1"))?;
        let version = advertised.min(6);
        if version < 2 {
            return Err(KwinError::Wayland(format!(
                "zkde_screencast_unstable_v1 v{advertised} is too old"
            )));
        }
        let screencast: ZkdeScreencastUnstableV1 = registry.bind(global, version, &qh, ());
        info!(version, monitors = ?*monitors.borrow(), "bound zkde_screencast_unstable_v1");

        std::thread::Builder::new()
            .name("slate-wayland".into())
            .spawn(move || loop {
                if let Err(e) = queue.blocking_dispatch(&mut state) {
                    warn!("wayland dispatch ended: {e}");
                    break;
                }
                state.publish();
            })?;

        Ok(Self {
            inner: Arc::new(Inner {
                conn,
                qh,
                screencast,
                version,
                monitors,
            }),
        })
    }

    /// Names of the connected monitors (every output except virtual ones), kept up to date.
    /// Disabled outputs are not listed; outputs whose screens are merely off (DPMS) are.
    pub fn monitors(&self) -> watch::Receiver<Vec<String>> {
        self.inner.monitors.clone()
    }

    /// Creates a virtual output. `width`/`height` are the mode size in physical pixels
    /// (KWin 6.7 treats them as such and ignores the scale argument; see m0-findings S1).
    pub async fn create(
        &self,
        name: &str,
        description: &str,
        width: u32,
        height: u32,
    ) -> Result<VirtualOutput> {
        let (tx, mut rx) = watch::channel(StreamStatus::Pending);
        let data = Arc::new(StreamData {
            status: tx,
            serial: std::sync::Mutex::new(None),
        });
        let i = &self.inner;
        let pointer: u32 = zsc::Pointer::Embedded.into();
        let (w, h) = (to_i32(width), to_i32(height));
        let stream = if i.version >= 4 {
            i.screencast.stream_virtual_output_with_description(
                name.to_owned(),
                description.to_owned(),
                w,
                h,
                1.0,
                pointer,
                &i.qh,
                data.clone(),
            )
        } else {
            i.screencast.stream_virtual_output(
                name.to_owned(),
                w,
                h,
                1.0,
                pointer,
                &i.qh,
                data.clone(),
            )
        };
        i.conn
            .flush()
            .map_err(|e| KwinError::Wayland(e.to_string()))?;
        debug!(name, width, height, "requested virtual output");

        let status = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let s = rx.borrow_and_update().clone();
                if s != StreamStatus::Pending {
                    return s;
                }
                if rx.changed().await.is_err() {
                    return StreamStatus::Closed;
                }
            }
        })
        .await
        .map_err(|_| KwinError::Timeout("virtual output stream"))?;

        match status {
            StreamStatus::Ready { serial, node_id } => {
                let output = VirtualOutput {
                    manager: self.clone(),
                    stream,
                    output_name: format!("{KWIN_VIRTUAL_PREFIX}{name}"),
                    pipewire_serial: serial,
                    pipewire_node: node_id,
                    status: rx,
                };
                info!(output = %output.output_name, ?serial, node_id, "virtual output ready");
                Ok(output)
            }
            StreamStatus::Failed(e) => {
                stream.close();
                let _ = i.conn.flush();
                Err(KwinError::OutputFailed(e))
            }
            StreamStatus::Closed | StreamStatus::Pending => {
                stream.close();
                let _ = i.conn.flush();
                Err(KwinError::OutputFailed(
                    "stream closed before it was created".into(),
                ))
            }
        }
    }
}

fn to_i32(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// A live virtual output. Dropping it removes the output from KWin.
#[derive(Debug)]
pub struct VirtualOutput {
    manager: VirtualOutputManager,
    stream: ZkdeScreencastStreamUnstableV1,
    /// Output (connector) name as seen in Display settings / kscreen-doctor / EIS regions.
    pub output_name: String,
    /// PipeWire object serial (use as `target-object` for `pipewiresrc`), when KWin provides it.
    pub pipewire_serial: Option<u64>,
    /// PipeWire node id (deprecated by KWin in favour of the serial).
    pub pipewire_node: u32,
    status: watch::Receiver<StreamStatus>,
}

impl VirtualOutput {
    /// `pipewiresrc target-object` value.
    pub fn pipewire_target(&self) -> String {
        match self.pipewire_serial {
            Some(s) => s.to_string(),
            None => self.pipewire_node.to_string(),
        }
    }

    /// Resolves when KWin closes the stream (e.g. the user disabled the output).
    pub async fn closed(&mut self) {
        loop {
            if matches!(
                *self.status.borrow_and_update(),
                StreamStatus::Closed | StreamStatus::Failed(_)
            ) {
                return;
            }
            if self.status.changed().await.is_err() {
                return;
            }
        }
    }
}

impl Drop for VirtualOutput {
    fn drop(&mut self) {
        self.stream.close();
        let _ = self.manager.inner.conn.flush();
        info!(output = %self.output_name, "virtual output removed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitors_exclude_virtual_outputs_and_keep_unnamed_ones() {
        let outputs = [
            (7, Some("Virtual-PlasmaSlate-01234567")),
            (3, Some("HDMI-A-1")),
            (2, Some("DP-1")),
            (9, None),
        ];
        assert_eq!(
            monitor_names(outputs.into_iter()),
            vec!["DP-1", "HDMI-A-1", "wl_output@9"]
        );
        assert!(monitor_names([(7, Some("Virtual-RateTest"))].into_iter()).is_empty());
    }
}
