//! KWin integration for Plasma Slate.
//!
//! * [`virtual_output`] — create/destroy KWin virtual outputs (`zkde_screencast_unstable_v1`) and
//!   obtain their PipeWire stream (ADR 0001).
//! * [`eis`] — inject touch through KWin's EIS endpoint (ADR 0002).
//! * [`kscreen`] — read and change output configuration (mode, position, scale) via `kscreen-doctor`.
//!
//! All KWin-specific knowledge lives in this crate.

pub mod eis;
pub mod kscreen;
pub mod virtual_output;

/// Errors from KWin integration.
#[derive(Debug, thiserror::Error)]
pub enum KwinError {
    #[error("wayland: {0}")]
    Wayland(String),
    #[error(
        "KWin did not expose {0} to this executable; install the plasma-slate .desktop file \
         (see packaging/ or scripts/install-dev-desktop-files.sh)"
    )]
    Restricted(&'static str),
    #[error("virtual output failed: {0}")]
    OutputFailed(String),
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    #[error("d-bus: {0}")]
    DBus(#[from] zbus::Error),
    #[error("eis: {0}")]
    Eis(String),
    #[error("kscreen-doctor: {0}")]
    Kscreen(String),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = KwinError> = std::result::Result<T, E>;
