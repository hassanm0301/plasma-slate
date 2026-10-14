//! DNS-SD advertisement of the daemon on the local network.

use std::collections::HashMap;

use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};
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
    // Android 10 resolves a single address per service: never offer addresses the tablet can't
    // reach (container bridges, VPN tunnels) or IPv6 link-local ones Java can't connect to.
    let _ = daemon.disable_interface(IfKind::IPv6);
    for name in virtual_interfaces() {
        let _ = daemon.disable_interface(IfKind::Name(name));
    }
    // A PC on both Ethernet and Wi-Fi in the same subnet: advertise only the wired address, so
    // the stream doesn't take two wireless hops.
    for name in redundant_wireless_interfaces() {
        info!(interface = %name, "not advertising on Wi-Fi: a wired interface is on the same network");
        let _ = daemon.disable_interface(IfKind::Name(name));
    }
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

/// Interface name prefixes that are never useful for reaching a tablet on the LAN.
const VIRTUAL_PREFIXES: &[&str] = &["lo", "docker", "br-", "veth", "virbr", "vmnet", "vboxnet", "podman", "cni", "flannel", "tun", "tap", "wg", "zt"];

pub fn is_virtual_interface(name: &str) -> bool {
    VIRTUAL_PREFIXES.iter().any(|p| name.starts_with(p))
}

fn virtual_interfaces() -> Vec<String> {
    std::fs::read_dir("/sys/class/net")
        .map(|it| {
            it.filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|n| is_virtual_interface(n))
                .collect()
        })
        .unwrap_or_default()
}

fn is_wireless(name: &str) -> bool {
    std::path::Path::new("/sys/class/net").join(name).join("wireless").exists()
}

/// IPv4 network address of an interface address.
fn network(ip: std::net::Ipv4Addr, prefix: u8) -> u32 {
    let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(prefix.min(32))) };
    u32::from(ip) & mask
}

/// Picks wireless interfaces that share an IPv4 subnet with a wired one.
pub fn redundant_wireless(ifaces: &[(String, std::net::Ipv4Addr, u8, bool)]) -> Vec<String> {
    let mut out: Vec<String> = ifaces
        .iter()
        .filter(|(n, ip, p, wireless)| {
            *wireless
                && !is_virtual_interface(n)
                && ifaces.iter().any(|(n2, ip2, p2, w2)| !*w2 && !is_virtual_interface(n2) && p2 == p && network(*ip2, *p2) == network(*ip, *p))
        })
        .map(|(n, ..)| n.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn redundant_wireless_interfaces() -> Vec<String> {
    let Ok(addrs) = if_addrs::get_if_addrs() else { return Vec::new() };
    let v4: Vec<(String, std::net::Ipv4Addr, u8, bool)> = addrs
        .into_iter()
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(a) => {
                let prefix = u32::from(a.netmask).count_ones() as u8;
                Some((i.name.clone(), a.ip, prefix, is_wireless(&i.name)))
            }
            _ => None,
        })
        .collect();
    redundant_wireless(&v4)
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
    use super::{is_virtual_interface, sanitize};

    #[test]
    fn prefers_wired_on_same_subnet() {
        use std::net::Ipv4Addr;
        let i = |n: &str, ip: [u8; 4], p: u8, w: bool| (n.to_string(), Ipv4Addr::from(ip), p, w);
        let ifs = vec![
            i("enp9s0", [192, 168, 100, 185], 24, false),
            i("wlp10s0", [192, 168, 100, 173], 24, true),
            i("wlan1", [10, 0, 0, 5], 24, true),
            i("docker0", [172, 17, 0, 1], 16, false),
        ];
        assert_eq!(super::redundant_wireless(&ifs), vec!["wlp10s0".to_string()]);
    }

    #[test]
    fn filters_virtual_interfaces() {
        for n in ["docker0", "br-1614fedd3176", "veth12ab", "virbr0", "lo", "wg0"] {
            assert!(is_virtual_interface(n), "{n}");
        }
        for n in ["enp9s0", "wlp10s0", "eth0", "wlan0", "tailscale0"] {
            assert!(!is_virtual_interface(n), "{n}");
        }
    }

    #[test]
    fn sanitizes_labels() {
        assert_eq!(sanitize("archlinux"), "archlinux");
        assert_eq!(sanitize("Hassan's PC"), "Hassan-s-PC");
        assert_eq!(sanitize("..."), "plasma-slate");
    }
}
