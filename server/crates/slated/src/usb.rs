//! USB transport via `adb reverse`: a tablet plugged in with USB debugging enabled can reach
//! the daemon at `127.0.0.1:<port>` without any network.

use std::collections::HashSet;
use std::time::Duration;

use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Parses `adb devices` output into serials in the `device` state.
pub fn parse_adb_devices(out: &str) -> Vec<String> {
    out.lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let serial = it.next()?;
            (it.next()? == "device").then(|| serial.to_string())
        })
        .collect()
}

const APP_ID: &str = "io.github.hassanm0301.plasmaslate";

/// Opens the tablet app connected over USB (only when it is installed).
async fn launch_app(serial: &str, port: u16) {
    let installed = Command::new("adb")
        .args(["-s", serial, "shell", "pm", "path", APP_ID])
        .output()
        .await
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("package:"))
        .unwrap_or(false);
    if !installed {
        debug!(%serial, "Plasma Slate app not installed on USB device");
        return;
    }
    let component = format!("{APP_ID}/.MainActivity");
    let port = port.to_string();
    let args = [
        "-s",
        serial,
        "shell",
        "am",
        "start",
        "-n",
        &component,
        "--es",
        "connect_host",
        "127.0.0.1",
        "--ei",
        "connect_port",
        &port,
        "--ez",
        "connect_usb",
        "true",
    ];
    match Command::new("adb").args(args).output().await {
        Ok(o) if o.status.success() => info!(%serial, "USB: opened the tablet app"),
        Ok(o) => {
            warn!(%serial, "could not open the tablet app: {}", String::from_utf8_lossy(&o.stderr).trim())
        }
        Err(e) => warn!(%serial, "could not open the tablet app: {e}"),
    }
}

/// Keeps `adb reverse tcp:port tcp:port` set up for every attached device and, when
/// `autostart` is set, opens the tablet app when a device is plugged in.
pub async fn run(port: u16, autostart: bool, shutdown: CancellationToken) {
    if Command::new("adb").arg("version").output().await.is_err() {
        info!("adb not found; USB connections disabled");
        return;
    }
    let mut known: HashSet<String> = HashSet::new();
    let mut tick = tokio::time::interval(Duration::from_secs(3));
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tick.tick() => {}
        }
        let Ok(out) = Command::new("adb").arg("devices").output().await else {
            continue;
        };
        let devices: HashSet<String> = parse_adb_devices(&String::from_utf8_lossy(&out.stdout))
            .into_iter()
            .collect();
        for serial in devices.difference(&known) {
            let spec = format!("tcp:{port}");
            match Command::new("adb")
                .args(["-s", serial, "reverse", &spec, &spec])
                .output()
                .await
            {
                Ok(o) if o.status.success() => {
                    info!(%serial, port, "USB: adb reverse set up");
                    if autostart {
                        launch_app(serial, port).await;
                    }
                }
                Ok(o) => {
                    warn!(%serial, "adb reverse failed: {}", String::from_utf8_lossy(&o.stderr).trim())
                }
                Err(e) => warn!(%serial, "adb reverse: {e}"),
            }
        }
        for serial in known.difference(&devices) {
            debug!(%serial, "USB device gone");
        }
        known = devices;
    }
    for serial in &known {
        let _ = Command::new("adb")
            .args(["-s", serial, "reverse", "--remove", &format!("tcp:{port}")])
            .output()
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_device_list() {
        let out = "* daemon started successfully\nList of devices attached\nN2PNU21318204407\tdevice\nemulator-5554\toffline\nABC\tunauthorized\n\n";
        assert_eq!(parse_adb_devices(out), vec!["N2PNU21318204407".to_string()]);
    }
}
