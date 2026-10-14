//! Networking building blocks for the Plasma Slate daemon.
//!
//! * [`identity`] — persistent self-signed server certificate + fingerprints.
//! * [`tls`] — TLS 1.3 acceptor built from the identity.
//! * [`mdns`] — DNS-SD advertisement (`_plasmaslate._tcp`).

pub mod identity;
pub mod mdns;
pub mod tls;

/// Networking errors.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("certificate: {0}")]
    Cert(String),
    #[error("tls: {0}")]
    Tls(#[from] rustls::Error),
    #[error("mdns: {0}")]
    Mdns(String),
}

pub type Result<T, E = NetError> = std::result::Result<T, E>;
