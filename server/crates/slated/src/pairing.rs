//! Pairing approval through a desktop notification with "Allow" / "Deny" buttons
//! (org.freedesktop.Notifications, rendered by Plasma).

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use tracing::{info, warn};
use zbus::zvariant::Value;

use crate::backend::Approver;

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
}

/// Asks through a notification.
#[derive(Debug)]
pub struct NotificationApprover {
    pub timeout: Duration,
}

#[async_trait]
impl Approver for NotificationApprover {
    async fn approve(&self, device_name: &str, device_id: &str, server_fingerprint: &str) -> bool {
        match self.ask(device_name, device_id, server_fingerprint).await {
            Ok(v) => v,
            Err(e) => {
                warn!("pairing prompt failed: {e}");
                false
            }
        }
    }
}

impl NotificationApprover {
    async fn ask(&self, device_name: &str, device_id: &str, fp: &str) -> zbus::Result<bool> {
        let conn = zbus::Connection::session().await?;
        let proxy = NotificationsProxy::new(&conn).await?;
        let mut actions = proxy.receive_action_invoked().await?;
        let mut closed = proxy.receive_notification_closed().await?;
        let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
        hints.insert("urgency", Value::U8(2));
        hints.insert(
            "desktop-entry",
            Value::from("io.github.hassanm0301.plasmaslate"),
        );
        let short_id: String = device_id.chars().take(8).collect();
        let body = format!(
            "<b>{}</b> wants to connect as an extra screen with touch input.<br/>Device {short_id} · PC fingerprint {fp}",
            escape(device_name)
        );
        let id = proxy
            .notify(
                "Plasma Slate",
                0,
                "video-display",
                "Allow tablet to connect?",
                &body,
                &["allow", "Allow", "deny", "Deny"],
                hints,
                i32::try_from(self.timeout.as_millis()).unwrap_or(i32::MAX),
            )
            .await?;
        info!(device_name, device_id, "pairing prompt shown");

        let decision = tokio::time::timeout(self.timeout, async {
            loop {
                tokio::select! {
                    Some(sig) = actions.next() => {
                        if let Ok(args) = sig.args() {
                            if args.id == id {
                                return args.action_key == "allow";
                            }
                        }
                    }
                    Some(sig) = closed.next() => {
                        if let Ok(args) = sig.args() {
                            if args.id == id {
                                return false;
                            }
                        }
                    }
                    else => return false,
                }
            }
        })
        .await
        .unwrap_or(false);
        let _ = proxy.close_notification(id).await;
        info!(device_name, allowed = decision, "pairing decision");
        Ok(decision)
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
