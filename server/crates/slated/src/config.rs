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
    /// Open the tablet app automatically when a tablet is plugged in over USB.
    pub usb_autostart: bool,
    /// Override the automatic bitrate (kbit/s).
    pub bitrate_kbps: Option<u32>,
    /// Codec choice: "auto" (the tablet's preference order), "h264" or "hevc".
    pub codec: String,
    /// Maximum simultaneously connected tablets.
    pub max_sessions: usize,
    /// Approve new devices without asking (only for automated testing).
    pub auto_approve: bool,
    /// Seconds to wait for the user to answer a pairing prompt.
    pub pairing_timeout_secs: u64,
    /// Default stream resolution for tablets without their own setting:
    /// `native`, `NN%` or `WIDTHxHEIGHT` (aspect ratio is kept).
    pub resolution: String,
    /// Default frames per second; 0 = the tablet's refresh rate.
    pub fps: u32,
    /// Force software encoding.
    pub software_encoding: bool,

    // --- latency tuning (see README "Latency tuning")
    /// Hardware encoder speed preset (VA `target-usage`, 1–7); 0 = the fastest for this GPU.
    /// The scale is driver-specific: Intel is fastest at 7, Mesa/AMD at 1.
    pub target_usage: u32,
    /// Seconds between periodic keyframes; 0 = none (keyframes only when the tablet asks).
    /// Frames travel over TCP, so periodic keyframes only add size spikes.
    pub keyframe_interval_secs: u32,
    /// Encoder buffer (CPB) in frames; 0 = encoder default. 1–2 keeps every frame near
    /// bitrate/fps so a keyframe or a big change never bursts over several frames, at the
    /// price of a blurrier first frame after one.
    pub cpb_frames: u32,
    /// Insert access-unit delimiters in the stream (some decoders find frame ends faster).
    pub aud: bool,
    /// `cbr`: every frame is padded to bitrate/fps, a constant stream (the Mesa/AMD encoder
    /// pads even a static screen). `vbr`: frames are as small as the content allows, bitrate
    /// is the ceiling. `auto`: cbr, or vbr when `filler_frames` > 0 (otherwise the fillers would
    /// multiply the bandwidth).
    pub rate_control: String,
    /// Repeats of the last picture sent between real frames (0–3). Hardware decoders that
    /// hold a couple of frames release them sooner when more input arrives; the PC encodes
    /// (fps × (1 + filler_frames)) frames per second, so check the encoder keeps up
    /// (`slated doctor` prints its speed).
    pub filler_frames: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: slate_protocol::DEFAULT_PORT,
            bind: "0.0.0.0".into(),
            server_name: None,
            mdns: true,
            usb_adb: true,
            usb_autostart: true,
            bitrate_kbps: None,
            codec: "auto".into(),
            max_sessions: 4,
            auto_approve: false,
            pairing_timeout_secs: 60,
            software_encoding: false,
            resolution: "native".into(),
            fps: 0,
            target_usage: 0,
            keyframe_interval_secs: 0,
            cpb_frames: 0,
            aud: false,
            filler_frames: 0,
            rate_control: "auto".into(),
        }
    }
}

impl Config {
    /// Loads the config file; a missing file yields defaults.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                Ok(toml::from_str(&s).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn server_name(&self) -> String {
        self.server_name.clone().unwrap_or_else(hostname)
    }

    /// Default stream settings (invalid values fall back to native / auto).
    pub fn default_stream(&self) -> crate::stream_settings::StreamSettings {
        crate::stream_settings::StreamSettings {
            resolution: self.resolution.clone(),
            fps: self.fps,
        }
        .validated()
        .unwrap_or_else(|e| {
            tracing::warn!("slated.toml: {e}; using native resolution and refresh rate");
            Default::default()
        })
    }

    /// Encoder rate control for a stream with `filler_frames` fillers (see the field docs).
    pub fn rate_control(&self, filler_frames: u32) -> slate_media::RateControl {
        match self.rate_control.to_ascii_lowercase().as_str() {
            "cbr" => slate_media::RateControl::Cbr,
            "vbr" => slate_media::RateControl::Vbr,
            _ if filler_frames > 0 => slate_media::RateControl::Vbr,
            _ => slate_media::RateControl::Cbr,
        }
    }

    /// `Some(true)` = HEVC, `Some(false)` = H.264, `None` = follow the client.
    pub fn codec_preference(&self) -> Option<bool> {
        match self.codec.to_ascii_lowercase().as_str() {
            "hevc" | "h265" => Some(true),
            "h264" | "avc" => Some(false),
            _ => None,
        }
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
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        Self {
            config_file: config.join("plasma-slate/slated.toml"),
            data_dir: data.join("plasma-slate"),
        }
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
        assert_eq!(c.codec_preference(), Some(true));
        assert_eq!(Config::default().codec_preference(), None);
        assert!(!c.mdns);
        assert_eq!(c.max_sessions, 4);
    }

    #[test]
    fn stream_defaults() {
        let c: Config = toml::from_str("resolution = \"75%\"\nfps = 30").unwrap();
        let s = c.default_stream();
        assert_eq!((s.resolution.as_str(), s.fps), ("75%", 30));
        let bad: Config = toml::from_str("resolution = \"huge\"").unwrap();
        assert!(bad.default_stream().is_default());
    }

    #[test]
    fn latency_options() {
        let c: Config =
            toml::from_str("filler_frames = 2\ncpb_frames = 1\ntarget_usage = 3\naud = true")
                .unwrap();
        assert_eq!((c.filler_frames, c.cpb_frames, c.target_usage), (2, 1, 3));
        assert!(c.aud);
        let d = Config::default();
        assert_eq!((d.filler_frames, d.keyframe_interval_secs), (0, 0));
        assert_eq!(d.rate_control(0), slate_media::RateControl::Cbr);
        assert_eq!(d.rate_control(1), slate_media::RateControl::Vbr);
        let v: Config = toml::from_str("rate_control = \"VBR\"").unwrap();
        assert_eq!(v.rate_control(0), slate_media::RateControl::Vbr);
        let c: Config = toml::from_str("rate_control = \"cbr\"\nfiller_frames = 2").unwrap();
        assert_eq!(c.rate_control(2), slate_media::RateControl::Cbr);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<Config>("prot = 1").is_err());
    }
}
