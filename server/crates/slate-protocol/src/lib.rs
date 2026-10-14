//! Plasma Slate wire protocol.
//!
//! * [`pb`] — types generated from `proto/plasmaslate/v1/slate.proto`.
//! * [`framing`] — big-endian u32 length-prefixed `Envelope` frames over any async byte stream.
//! * [`touch`] — mapping of normalized touch coordinates into output geometry.

#![allow(missing_debug_implementations)]

pub mod framing;
pub mod touch;

/// Generated protobuf types.
#[allow(clippy::all, clippy::pedantic, missing_docs)]
pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/plasmaslate.v1.rs"));
}

pub use pb::envelope::Msg;
pub use pb::Envelope;

/// Wire protocol version implemented by this crate.
pub const PROTOCOL_VERSION: u32 = pb::ProtocolVersion::Current as u32;

/// Maximum accepted frame size (one Envelope), in bytes.
pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;

/// Default TCP port of the daemon.
pub const DEFAULT_PORT: u16 = 47_420;

/// mDNS / DNS-SD service type advertised by the daemon.
pub const MDNS_SERVICE_TYPE: &str = "_plasmaslate._tcp.local.";

impl Envelope {
    /// Wraps a message into an envelope.
    pub fn new(msg: Msg) -> Self {
        Self { msg: Some(msg) }
    }

    /// Short name of the contained message, for logs.
    pub fn kind(&self) -> &'static str {
        match &self.msg {
            None => "empty",
            Some(Msg::ClientHello(_)) => "ClientHello",
            Some(Msg::ServerHello(_)) => "ServerHello",
            Some(Msg::PairingPending(_)) => "PairingPending",
            Some(Msg::Error(_)) => "Error",
            Some(Msg::Ready(_)) => "Ready",
            Some(Msg::VideoFrame(_)) => "VideoFrame",
            Some(Msg::TouchEvent(_)) => "TouchEvent",
            Some(Msg::Ping(_)) => "Ping",
            Some(Msg::Pong(_)) => "Pong",
            Some(Msg::RequestKeyframe(_)) => "RequestKeyframe",
            Some(Msg::ClientStats(_)) => "ClientStats",
            Some(Msg::StreamChanged(_)) => "StreamChanged",
            Some(Msg::Goodbye(_)) => "Goodbye",
        }
    }
}

impl From<pb::Error> for Envelope {
    fn from(e: pb::Error) -> Self {
        Envelope::new(Msg::Error(e))
    }
}

/// Convenience constructor for an error envelope.
pub fn error(code: pb::ErrorCode, message: impl Into<String>) -> Envelope {
    pb::Error {
        code: code as i32,
        message: message.into(),
    }
    .into()
}
