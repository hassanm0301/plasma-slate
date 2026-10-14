//! Touch injection through KWin's EIS endpoint (`org.kde.KWin.EIS.RemoteDesktop`).
//!
//! One [`TouchInjector`] per daemon. A background thread consumes EIS events (seat, devices,
//! regions, resume/pause) and keeps the current touch device; injection calls may come from
//! any thread.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use reis::ei;
use reis::event::{Device, DeviceCapability, EiEvent};
use tracing::{debug, info, warn};

use crate::{KwinError, Result};

/// `connectToEIS` capability mask for touch only (1 keyboard, 2 pointer, 4 touch).
const CAP_TOUCH: i32 = 4;

/// One output region as reported by KWin for the touch device (global logical pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug)]
struct Active {
    device: Device,
    touch: ei::Touchscreen,
    serial: u32,
    emulating: bool,
}

#[derive(Debug, Default)]
struct Shared {
    active: Option<Active>,
    regions: HashMap<String, Region>,
    sequence: u32,
    /// Touch ids currently down, so they can be released if the device goes away.
    down: std::collections::HashSet<u32>,
}

/// Injects touch events into KWin.
#[derive(Debug, Clone)]
pub struct TouchInjector {
    ctx: ei::Context,
    shared: Arc<Mutex<Shared>>,
    start: Instant,
    _dbus: zbus::Connection,
}

/// One touch action.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TouchAction {
    Down { id: u32, x: f64, y: f64 },
    Motion { id: u32, x: f64, y: f64 },
    Up { id: u32 },
    Cancel { id: u32 },
}

impl TouchInjector {
    /// Connects to KWin and waits (up to 3 s) for the touch device to be resumed.
    pub async fn connect() -> Result<Self> {
        let dbus = zbus::Connection::session().await?;
        let reply = dbus
            .call_method(
                Some("org.kde.KWin"),
                "/org/kde/KWin/EIS/RemoteDesktop",
                Some("org.kde.KWin.EIS.RemoteDesktop"),
                "connectToEIS",
                &(CAP_TOUCH,),
            )
            .await
            .map_err(|e| match e {
                zbus::Error::MethodError(name, _, _) if name.as_str().contains("AccessDenied") => {
                    KwinError::Restricted("org.kde.KWin.EIS.RemoteDesktop")
                }
                other => KwinError::DBus(other),
            })?;
        let (fd, cookie): (zbus::zvariant::OwnedFd, i32) = reply
            .body()
            .deserialize()
            .map_err(|e| KwinError::Eis(e.to_string()))?;
        let fd: OwnedFd = fd.into();
        let ctx = ei::Context::new(UnixStream::from(fd))?;
        info!(cookie, "connected to KWin EIS");

        let shared = Arc::new(Mutex::new(Shared::default()));
        let (ready_tx, ready_rx) =
            tokio::sync::oneshot::channel::<std::result::Result<(), String>>();
        {
            let shared = shared.clone();
            let ctx = ctx.clone();
            // The event iterator is !Send, so the handshake happens on the event thread.
            std::thread::Builder::new()
                .name("slate-eis".into())
                .spawn(move || {
                    match ctx.handshake_blocking("plasma-slate", ei::handshake::ContextType::Sender)
                    {
                        Ok((_conn, events)) => run_events(events, &ctx, &shared, Some(ready_tx)),
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.to_string()));
                        }
                    }
                })?;
        }
        match tokio::time::timeout(std::time::Duration::from_secs(3), ready_rx).await {
            Ok(Ok(Err(e))) => return Err(KwinError::Eis(format!("handshake: {e}"))),
            Ok(_) => {}
            Err(_) => warn!("EIS touch device not resumed yet; continuing"),
        }

        Ok(Self {
            ctx,
            shared,
            start: Instant::now(),
            _dbus: dbus,
        })
    }

    /// Current region of the output with the given name, as known to EIS.
    pub fn region(&self, output_name: &str) -> Option<Region> {
        self.shared.lock().ok()?.regions.get(output_name).copied()
    }

    /// Whether a touch device is available and resumed.
    pub fn is_ready(&self) -> bool {
        self.shared
            .lock()
            .map(|s| s.active.is_some())
            .unwrap_or(false)
    }

    /// Sends a batch of actions as one EIS frame (one hardware "report").
    pub fn send(&self, actions: &[TouchAction]) -> Result<()> {
        let mut s = self
            .shared
            .lock()
            .map_err(|_| KwinError::Eis("poisoned".into()))?;
        let Shared {
            active,
            sequence,
            down,
            ..
        } = &mut *s;
        let Some(a) = active.as_mut() else {
            return Err(KwinError::Eis("no touch device".into()));
        };
        if !a.emulating {
            *sequence = sequence.wrapping_add(1);
            a.device.device().start_emulating(a.serial, *sequence);
            a.emulating = true;
        }
        for act in actions {
            match *act {
                TouchAction::Down { id, x, y } => {
                    a.touch.down(id, x as f32, y as f32);
                    down.insert(id);
                }
                TouchAction::Motion { id, x, y } => a.touch.motion(id, x as f32, y as f32),
                TouchAction::Up { id } => {
                    a.touch.up(id);
                    down.remove(&id);
                }
                TouchAction::Cancel { id } => {
                    // ei_touchscreen.cancel needs interface v2; up is the safe fallback.
                    if a.touch.version() >= 2 {
                        a.touch.cancel(id);
                    } else {
                        a.touch.up(id);
                    }
                    down.remove(&id);
                }
            }
        }
        let us = u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX);
        a.device.device().frame(a.serial, us);
        drop(s);
        self.ctx
            .flush()
            .map_err(|e| KwinError::Eis(e.to_string()))?;
        Ok(())
    }

    /// Releases every touch point whose id satisfies `pred` (used when a session ends).
    pub fn release_where(&self, pred: impl Fn(u32) -> bool) {
        let ids: Vec<u32> = match self.shared.lock() {
            Ok(s) => s.down.iter().copied().filter(|id| pred(*id)).collect(),
            Err(_) => return,
        };
        if !ids.is_empty() {
            let actions: Vec<_> = ids
                .into_iter()
                .map(|id| TouchAction::Cancel { id })
                .collect();
            let _ = self.send(&actions);
        }
    }
}

fn run_events(
    events: impl Iterator<Item = std::result::Result<EiEvent, reis::Error>>,
    ctx: &ei::Context,
    shared: &Arc<Mutex<Shared>>,
    mut ready: Option<tokio::sync::oneshot::Sender<std::result::Result<(), String>>>,
) {
    for ev in events {
        let ev = match ev {
            Ok(ev) => ev,
            Err(e) => {
                warn!("EIS event error: {e}");
                break;
            }
        };
        let Ok(mut s) = shared.lock() else { break };
        match ev {
            EiEvent::SeatAdded(seat) => {
                seat.seat.bind_capabilities(DeviceCapability::Touch.into());
                let _ = ctx.flush();
            }
            EiEvent::DeviceAdded(d) => {
                debug!(name = ?d.device.name(), regions = ?d.device.regions(), "EIS device added");
                if d.device.has_capability(DeviceCapability::Touch) {
                    s.regions = regions_of(&d.device);
                }
            }
            EiEvent::DeviceResumed(r) => {
                if r.device.has_capability(DeviceCapability::Touch) {
                    if let Some(touch) = r.device.interface::<ei::Touchscreen>() {
                        s.regions = regions_of(&r.device);
                        info!(regions = ?s.regions.keys().collect::<Vec<_>>(), "EIS touch device resumed");
                        s.active = Some(Active {
                            device: r.device.clone(),
                            touch,
                            serial: r.serial,
                            emulating: false,
                        });
                        if let Some(tx) = ready.take() {
                            let _ = tx.send(Ok(()));
                        }
                    }
                }
            }
            EiEvent::DevicePaused(p) => {
                if s.active.as_ref().is_some_and(|a| a.device == p.device) {
                    debug!("EIS touch device paused");
                    s.active = None;
                    s.down.clear();
                }
            }
            EiEvent::DeviceRemoved(r) => {
                if s.active.as_ref().is_some_and(|a| a.device == r.device) {
                    debug!("EIS touch device removed (layout change?)");
                    s.active = None;
                    s.down.clear();
                }
            }
            EiEvent::Disconnected(d) => {
                warn!(?d, "EIS disconnected");
                s.active = None;
                break;
            }
            _ => {}
        }
    }
}

fn regions_of(device: &Device) -> HashMap<String, Region> {
    device
        .regions()
        .iter()
        .filter_map(|r| {
            let name = r.mapping_id.clone()?;
            Some((
                name,
                Region {
                    x: f64::from(r.x),
                    y: f64::from(r.y),
                    width: f64::from(r.width),
                    height: f64::from(r.height),
                },
            ))
        })
        .collect()
}
