//! DNS-SD advertisement of the daemon on the local network.

use std::collections::HashMap;

use mdns_sd::{ServiceDaemon, ServiceInfo};
use tracing::{info, warn};

use crate::{NetError, Result};

/// mDNS service type.
pub const SERVICE_TYPE: &str = "_plasmaslate._tcp.local.";

/// Keeps the advertisement alive; unregisters on drop.
pub struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl std::fmt::Debug for Advertisement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Advertisement").field("fullname", &self.fullname).finish()
    }
}

/// TXT record contents.
#[derive(Debug, Clone)]
pub struct TxtInfo {
    /// Stable server id.
    pub server_id: String,
    /// Pretty name shown in the tablet's server list.
    pub name: String,
    /// Short certificate fingerprint.
    pub fingerprint: String,
    pub protocol_version: u32,
}

/// Advertises `instance` on `port` on all interfaces.
pub fn advertise(instance: &str, host: &str, port: u16, txt: &TxtInfo) -> Result<Advertisement> {
    let daemon = ServiceDaemon::new().map_err(|e| NetError::Mdns(e.to_string()))?;
    let props: HashMap<String, String> = [
        ("id".to_string(), txt.server_id.clone()),
        ("name".to_string(), txt.name.clone()),
        ("fp".to_string(), txt.fingerprint.clone()),
        ("v".to_string(), txt.protocol_version.to_string()),
    ]
    .into_iter()
    .collect();
    let host_name = format!("{}.local.", sanitize(host));
    let info = ServiceInfo::new(SERVICE_TYPE, &sanitize(instance), &host_name, "", port, props)
        .map_err(|e| NetError::Mdns(e.to_string()))?
        .enable_addr_auto();
    let fullname = info.get_fullname().to_string();
    daemon.register(info).map_err(|e| NetError::Mdns(e.to_string()))?;
    info!(%fullname, port, "advertising via mDNS");
    Ok(Advertisement { daemon, fullname })
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        if let Err(e) = self.daemon.unregister(&self.fullname) {
            warn!("mdns unregister: {e}");
        }
        let _ = self.daemon.shutdown();
    }
}

/// DNS labels: keep it simple (letters, digits, '-').
fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    let t = t.trim_matches('-').to_string();
    if t.is_empty() {
        "plasma-slate".into()
    } else {
        t.chars().take(63).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn sanitizes_labels() {
        assert_eq!(sanitize("archlinux"), "archlinux");
        assert_eq!(sanitize("Hassan's PC"), "Hassan-s-PC");
        assert_eq!(sanitize("..."), "plasma-slate");
    }
}
