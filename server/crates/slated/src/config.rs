//! Daemon configuration (`~/.config/plasma-slate/slated.toml`) and data paths.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Daemon configuration. Every field has a default, so an empty or missing file is valid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// TCP port to listen on.
    pub port: u16,
    /// Address to bind (all interfaces by default).
    pub bind: String,
    /// Name shown on the tablet; defaults to the host name.
    pub server_name: Option<String>,
    /// Advertise on the LAN with mDNS.
    pub mdns: bool,
    /// Run `adb reverse` for USB-connected devices so they can connect without Wi-Fi.
    pub usb_adb: bool,
    /// Override the automatic bitrate (kbit/s).
    pub bitrate_kbps: Option<u32>,
    /// Preferred codec when the tablet supports several: "h264" or "hevc".
    pub codec: String,
    /// Maximum simultaneously connected tablets.
    pub max_sessions: usize,
    /// Approve new devices without asking (only for automated testing).
    pub auto_approve: bool,
    /// Seconds to wait for the user to answer a pairing prompt.
    pub pairing_timeout_secs: u64,
    /// Force software encoding.
    pub software_encoding: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: slate_protocol::DEFAULT_PORT,
            bind: "0.0.0.0".into(),
            server_name: None,
            mdns: true,
            usb_adb: true,
            bitrate_kbps: None,
            codec: "h264".into(),
            max_sessions: 4,
            auto_approve: false,
            pairing_timeout_secs: 60,
            software_encoding: false,
        }
    }
}

impl Config {
    /// Loads the config file; a missing file yields defaults.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(toml::from_str(&s).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn server_name(&self) -> String {
        self.server_name.clone().unwrap_or_else(hostname)
    }

    pub fn prefer_hevc(&self) -> bool {
        self.codec.eq_ignore_ascii_case("hevc") || self.codec.eq_ignore_ascii_case("h265")
    }
}

/// Machine host name.
pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "plasma-slate".into())
}

/// XDG locations used by the daemon.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
        let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
        Self { config_file: config.join("plasma-slate/slated.toml"), data_dir: data.join("plasma-slate") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_default() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn partial_file_overrides() {
        let c: Config = toml::from_str("port = 5000\ncodec = \"hevc\"\nmdns = false").unwrap();
        assert_eq!(c.port, 5000);
        assert!(c.prefer_hevc());
        assert!(!c.mdns);
        assert_eq!(c.max_sessions, 4);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<Config>("prot = 1").is_err());
    }
}
