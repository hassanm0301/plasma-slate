//! Persistent server identity: a self-signed ECDSA P-256 certificate.
//!
//! Clients pin the SHA-256 fingerprint of the DER certificate on first successful pairing
//! (trust on first use, confirmed by the user on the PC).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use tracing::info;

use crate::{NetError, Result};

/// Certificate + private key of the daemon.
#[derive(Debug)]
pub struct Identity {
    pub cert_der: CertificateDer<'static>,
    key_der: Vec<u8>,
    /// Lower-case hex SHA-256 of `cert_der`.
    pub fingerprint: String,
}

impl Identity {
    /// Loads `dir/server-cert.pem` + `dir/server-key.pem`, creating them on first run.
    pub fn load_or_create(dir: &Path, common_name: &str) -> Result<Self> {
        let cert_path = dir.join("server-cert.pem");
        let key_path = dir.join("server-key.pem");
        if cert_path.exists() && key_path.exists() {
            let cert_pem = fs::read_to_string(&cert_path)?;
            let key_pem = fs::read_to_string(&key_path)?;
            return Self::from_pem(&cert_pem, &key_pem);
        }
        fs::create_dir_all(dir)?;
        let (cert_pem, key_pem) = generate(common_name)?;
        write_private(&key_path, key_pem.as_bytes())?;
        fs::write(&cert_path, cert_pem.as_bytes())?;
        info!(path = %cert_path.display(), "created server certificate");
        Self::from_pem(&cert_pem, &key_pem)
    }

    /// Builds an identity from PEM strings.
    pub fn from_pem(cert_pem: &str, key_pem: &str) -> Result<Self> {
        let cert_der = pem_block(cert_pem, "CERTIFICATE")?;
        let key_der = pem_block(key_pem, "PRIVATE KEY")?;
        let fingerprint = hex(&Sha256::digest(&cert_der));
        Ok(Self {
            cert_der: CertificateDer::from(cert_der),
            key_der,
            fingerprint,
        })
    }

    /// Ephemeral identity (tests).
    pub fn ephemeral(common_name: &str) -> Result<Self> {
        let (c, k) = generate(common_name)?;
        Self::from_pem(&c, &k)
    }

    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der.clone()))
    }

    /// Human-friendly short form shown in pairing prompts, e.g. `3F2A-91C0`.
    pub fn short_fingerprint(&self) -> String {
        short_fingerprint(&self.fingerprint)
    }
}

/// `abcd1234...` → `ABCD-1234`.
pub fn short_fingerprint(hex_fp: &str) -> String {
    let up: String = hex_fp.chars().take(8).collect::<String>().to_uppercase();
    if up.len() < 8 {
        return up;
    }
    format!("{}-{}", &up[..4], &up[4..])
}

fn generate(common_name: &str) -> Result<(String, String)> {
    let mut params = rcgen::CertificateParams::new(vec!["plasma-slate.local".to_string()])
        .map_err(|e| NetError::Cert(e.to_string()))?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, common_name);
    params
        .distinguished_name
        .push(rcgen::DnType::OrganizationName, "Plasma Slate");
    // Long-lived: identity is pinned, not validated against a CA.
    params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    params.not_after = rcgen::date_time_ymd(2124, 1, 1);
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .map_err(|e| NetError::Cert(e.to_string()))?;
    let cert = params
        .self_signed(&key)
        .map_err(|e| NetError::Cert(e.to_string()))?;
    Ok((cert.pem(), key.serialize_pem()))
}

fn pem_block(pem: &str, label: &str) -> Result<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let start = pem
        .find(&begin)
        .ok_or_else(|| NetError::Cert(format!("missing {label} block")))?
        + begin.len();
    let stop = pem[start..]
        .find(&end)
        .ok_or_else(|| NetError::Cert(format!("unterminated {label} block")))?
        + start;
    let b64: String = pem[start..stop]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    base64_decode(&b64).ok_or_else(|| NetError::Cert("bad base64".into()))
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let bytes = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for &c in bytes {
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn write_private(path: &PathBuf, data: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_reload_keeps_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let a = Identity::load_or_create(dir.path(), "test").unwrap();
        let b = Identity::load_or_create(dir.path(), "test").unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.fingerprint.len(), 64);
        let mode = std::fs::metadata(dir.path().join("server-key.pem")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(mode.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn short_fp_format() {
        assert_eq!(short_fingerprint("3f2a91c0deadbeef"), "3F2A-91C0");
    }

    #[test]
    fn base64_roundtrip_known() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGk").unwrap(), b"hi");
        assert!(base64_decode("a$b").is_none());
    }
}
