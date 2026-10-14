//! Persistent state: paired devices, their auth tokens (hashed) and per-device placement.
//! Stored as JSON in `~/.local/share/plasma-slate/devices.json`, written atomically.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::backend::Placement;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub name: String,
    /// SHA-256 (hex) of the auth token issued at pairing.
    pub token_sha256: String,
    /// Unix seconds.
    pub paired_at: u64,
    pub last_seen: u64,
    /// Where the user placed this tablet's output (restored on connect).
    #[serde(default)]
    pub placement: Option<Placement>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    devices: BTreeMap<String, DeviceRecord>,
}

/// Device store.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    file: File,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sha256_hex(s: &str) -> String {
    slate_net::identity::hex(&Sha256::digest(s.as_bytes()))
}

/// Constant-time string comparison (both are hex digests of equal length).
fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

impl Store {
    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        let file = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, file })
    }

    fn save(&self) -> anyhow::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(serde_json::to_string_pretty(&self.file)?.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Checks a token presented by a device.
    pub fn verify(&self, device_id: &str, token: &str) -> bool {
        !token.is_empty()
            && self
                .file
                .devices
                .get(device_id)
                .is_some_and(|d| ct_eq(&d.token_sha256, &sha256_hex(token)))
    }

    /// Registers a newly paired device and returns the token to give it.
    pub fn pair(&mut self, device_id: &str, name: &str) -> anyhow::Result<String> {
        let token = slate_net::identity::hex(&rand::random::<[u8; 32]>());
        let placement = self.file.devices.get(device_id).and_then(|d| d.placement);
        self.file.devices.insert(
            device_id.to_string(),
            DeviceRecord {
                name: name.to_string(),
                token_sha256: sha256_hex(&token),
                paired_at: now(),
                last_seen: now(),
                placement,
            },
        );
        self.save()?;
        Ok(token)
    }

    pub fn touch(&mut self, device_id: &str, name: &str) -> anyhow::Result<()> {
        if let Some(d) = self.file.devices.get_mut(device_id) {
            d.last_seen = now();
            d.name = name.to_string();
            self.save()?;
        }
        Ok(())
    }

    pub fn placement(&self, device_id: &str) -> Option<Placement> {
        self.file.devices.get(device_id).and_then(|d| d.placement)
    }

    /// Stores the placement; returns whether it changed.
    pub fn set_placement(&mut self, device_id: &str, p: Placement) -> anyhow::Result<bool> {
        let Some(d) = self.file.devices.get_mut(device_id) else {
            return Ok(false);
        };
        if d.placement == Some(p) {
            return Ok(false);
        }
        d.placement = Some(p);
        self.save()?;
        Ok(true)
    }

    pub fn unpair(&mut self, device_id: &str) -> anyhow::Result<bool> {
        let removed = self.file.devices.remove(device_id).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    pub fn devices(&self) -> impl Iterator<Item = (&String, &DeviceRecord)> {
        self.file.devices.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_verify_persist_unpair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let mut s = Store::load(path.clone()).unwrap();
        let tok = s.pair("dev1", "Tablet").unwrap();
        assert!(s.verify("dev1", &tok));
        assert!(!s.verify("dev1", "wrong"));
        assert!(!s.verify("dev1", ""));
        assert!(!s.verify("dev2", &tok));

        assert!(s
            .set_placement(
                "dev1",
                Placement {
                    x: 10,
                    y: 20,
                    scale: 1.5
                }
            )
            .unwrap());
        assert!(!s
            .set_placement(
                "dev1",
                Placement {
                    x: 10,
                    y: 20,
                    scale: 1.5
                }
            )
            .unwrap());

        let s2 = Store::load(path.clone()).unwrap();
        assert!(s2.verify("dev1", &tok));
        assert_eq!(
            s2.placement("dev1"),
            Some(Placement {
                x: 10,
                y: 20,
                scale: 1.5
            })
        );
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains(&tok),
            "token must not be stored in clear text"
        );

        // Re-pairing keeps placement but rotates the token.
        let mut s3 = s2;
        let tok2 = s3.pair("dev1", "Tablet").unwrap();
        assert!(!s3.verify("dev1", &tok));
        assert!(s3.verify("dev1", &tok2));
        assert!(s3.placement("dev1").is_some());

        assert!(s3.unpair("dev1").unwrap());
        assert!(!s3.verify("dev1", &tok2));
    }
}
