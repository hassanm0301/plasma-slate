//! TLS acceptor (TLS 1.3 only, server-authenticated; clients authenticate in-band with a
//! pairing token).

use std::sync::Arc;

use tokio_rustls::TlsAcceptor;

use crate::identity::Identity;
use crate::Result;

/// Builds a TLS acceptor for the given identity.
pub fn acceptor(identity: &Identity) -> Result<TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![identity.cert_der.clone()], identity.private_key())?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// Client-side connector that accepts exactly one pinned certificate fingerprint
/// (or any certificate when `pinned` is `None`, recording what it saw). Used by tests and the
/// developer CLI client; the Android app implements the same policy.
pub mod client {
    use std::sync::{Arc, Mutex};

    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, SignatureScheme};
    use sha2::{Digest, Sha256};

    use crate::identity::hex;

    /// Pins a SHA-256 certificate fingerprint.
    #[derive(Debug)]
    pub struct PinnedVerifier {
        pinned: Option<String>,
        seen: Mutex<Option<String>>,
        provider: Arc<rustls::crypto::CryptoProvider>,
    }

    impl PinnedVerifier {
        pub fn new(pinned: Option<String>) -> Arc<Self> {
            Arc::new(Self { pinned, seen: Mutex::new(None), provider: Arc::new(rustls::crypto::ring::default_provider()) })
        }

        /// Fingerprint of the certificate presented by the server.
        pub fn seen(&self) -> Option<String> {
            self.seen.lock().ok().and_then(|g| g.clone())
        }
    }

    impl ServerCertVerifier for PinnedVerifier {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            let fp = hex(&Sha256::digest(end_entity.as_ref()));
            if let Ok(mut g) = self.seen.lock() {
                *g = Some(fp.clone());
            }
            match &self.pinned {
                Some(p) if *p != fp => Err(rustls::Error::General(format!("certificate fingerprint mismatch: {fp}"))),
                _ => Ok(ServerCertVerified::assertion()),
            }
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.provider.signature_verification_algorithms.supported_schemes()
        }
    }

    /// Builds a connector using a [`PinnedVerifier`].
    pub fn connector(verifier: Arc<PinnedVerifier>) -> crate::Result<tokio_rustls::TlsConnector> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::ServerName;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn pinned_handshake_and_mismatch() {
        let id = Identity::ephemeral("t").unwrap();
        let acc = acceptor(&id).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for _ in 0..2 {
                let (s, _) = listener.accept().await.unwrap();
                if let Ok(mut tls) = acc.accept(s).await {
                    let mut b = [0u8; 4];
                    tls.read_exact(&mut b).await.unwrap();
                    tls.write_all(&b).await.unwrap();
                    tls.flush().await.unwrap();
                }
            }
        });

        // Unpinned (first use): succeeds and reports the fingerprint.
        let v = client::PinnedVerifier::new(None);
        let c = client::connector(v.clone()).unwrap();
        let s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut tls = c.connect(ServerName::try_from("plasma-slate.local").unwrap(), s).await.unwrap();
        tls.write_all(b"ping").await.unwrap();
        let mut b = [0u8; 4];
        tls.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"ping");
        assert_eq!(v.seen().unwrap(), id.fingerprint);

        // Wrong pin: handshake fails.
        let v = client::PinnedVerifier::new(Some("00".repeat(32)));
        let c = client::connector(v).unwrap();
        let s = tokio::net::TcpStream::connect(addr).await.unwrap();
        assert!(c.connect(ServerName::try_from("plasma-slate.local").unwrap(), s).await.is_err());
    }
}
