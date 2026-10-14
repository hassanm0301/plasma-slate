//! Accept loop: TCP → TLS → session.

use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::session::{self, SessionContext};

/// Accepts connections until `shutdown` is cancelled.
pub async fn serve(
    listener: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    ctx: Arc<SessionContext>,
    shutdown: CancellationToken,
) {
    let addr = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_default();
    info!(%addr, "listening");
    loop {
        let (tcp, peer) = tokio::select! {
            _ = shutdown.cancelled() => break,
            r = listener.accept() => match r {
                Ok(v) => v,
                Err(e) => {
                    warn!("accept: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            },
        };
        let _ = tcp.set_nodelay(true);
        let acceptor = acceptor.clone();
        let ctx = ctx.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let tls =
                match tokio::time::timeout(Duration::from_secs(10), acceptor.accept(tcp)).await {
                    Ok(Ok(s)) => s,
                    Ok(Err(e)) => {
                        debug!(%peer, "TLS handshake failed: {e}");
                        return;
                    }
                    Err(_) => {
                        debug!(%peer, "TLS handshake timed out");
                        return;
                    }
                };
            session::run(tls, peer.to_string(), ctx, shutdown).await;
        });
    }
    info!("listener stopped");
}
