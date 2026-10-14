//! `slated` — the Plasma Slate daemon.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

mod backend;
mod client;
mod config;
mod dbus;
mod pairing;
mod server;
mod session;
mod store;
mod usb;

use backend::Backends;
use config::{Config, Paths};
use session::{Registry, SessionContext};

#[derive(Debug, Parser)]
#[command(name = "slated", version, about = "Use an Android tablet as an extra KDE Plasma monitor")]
struct Cli {
    /// Config file (default: ~/.config/plasma-slate/slated.toml).
    #[arg(long, global = true)]
    config: Option<std::path::PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Run the daemon (default).
    Run {
        /// Override the listening port.
        #[arg(long)]
        port: Option<u16>,
        /// Approve new devices without a prompt (testing only).
        #[arg(long)]
        auto_approve: bool,
    },
    /// Check that this system has everything the daemon needs.
    Doctor,
    /// List paired devices.
    Devices,
    /// Forget a paired device.
    Unpair { device_id: String },
    /// Developer test client (acts like the tablet app).
    Client(client::ClientArgs),
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("SLATE_LOG").unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).with_target(false).init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let cli = Cli::parse();
    let paths = Paths::from_env();
    let config_path = cli.config.clone().unwrap_or_else(|| paths.config_file.clone());
    let mut config = Config::load(&config_path)?;

    match cli.cmd.unwrap_or(Cmd::Run { port: None, auto_approve: false }) {
        Cmd::Run { port, auto_approve } => {
            if let Some(p) = port {
                config.port = p;
            }
            config.auto_approve |= auto_approve;
            run_daemon(config, paths).await
        }
        Cmd::Doctor => doctor().await,
        Cmd::Devices => {
            let store = store::Store::load(paths.data_dir.join("devices.json"))?;
            for (id, d) in store.devices() {
                println!("{id}\t{}\tlast seen {}\tplacement {:?}", d.name, d.last_seen, d.placement);
            }
            Ok(())
        }
        Cmd::Unpair { device_id } => {
            let mut store = store::Store::load(paths.data_dir.join("devices.json"))?;
            println!("{}", if store.unpair(&device_id)? { "removed" } else { "not found" });
            Ok(())
        }
        Cmd::Client(args) => client::run(args).await,
    }
}

async fn run_daemon(config: Config, paths: Paths) -> anyhow::Result<()> {
    let config = Arc::new(config);
    let server_name = config.server_name();
    let identity = slate_net::identity::Identity::load_or_create(&paths.data_dir, &server_name).context("server identity")?;
    let acceptor = slate_net::tls::acceptor(&identity)?;
    let store = Arc::new(Mutex::new(store::Store::load(paths.data_dir.join("devices.json"))?));
    let registry = Arc::new(Registry::default());

    if let Err(e) = slate_media::init() {
        warn!("GStreamer: {e}");
    }
    let outputs = backend::kwin::KwinOutputs::connect().context(
        "cannot create virtual outputs: is this a Plasma Wayland session and is the plasma-slate .desktop file installed?",
    )?;
    let touch = Arc::new(backend::kwin::KwinTouch::connect().await);
    let backends = Backends {
        outputs: Arc::new(outputs),
        touch: touch.clone(),
        encoders: Arc::new(backend::kwin::GstEncoders),
        approver: Arc::new(pairing::NotificationApprover { timeout: Duration::from_secs(config.pairing_timeout_secs) }),
    };

    let ctx = Arc::new(SessionContext {
        backends,
        store: store.clone(),
        config: config.clone(),
        server_name: server_name.clone(),
        fingerprint_short: identity.short_fingerprint(),
        registry: registry.clone(),
    });

    let shutdown = CancellationToken::new();
    let listener = tokio::net::TcpListener::bind((config.bind.as_str(), config.port)).await.with_context(|| format!("bind port {}", config.port))?;

    let _dbus = match dbus::start(dbus::DaemonIface {
        registry: registry.clone(),
        store: store.clone(),
        server_name: server_name.clone(),
        fingerprint: identity.short_fingerprint(),
        port: config.port,
    })
    .await
    {
        Ok(c) => Some(c),
        Err(e) => {
            warn!("D-Bus API unavailable (another slated running?): {e}");
            None
        }
    };

    let _mdns = if config.mdns {
        let txt = slate_net::mdns::TxtInfo {
            server_id: identity.fingerprint.chars().take(16).collect(),
            name: server_name.clone(),
            fingerprint: identity.short_fingerprint(),
            protocol_version: slate_protocol::PROTOCOL_VERSION,
        };
        match slate_net::mdns::advertise(&server_name, &config::hostname(), config.port, &txt) {
            Ok(a) => Some(a),
            Err(e) => {
                warn!("mDNS advertisement failed: {e}");
                None
            }
        }
    } else {
        None
    };

    if config.usb_adb {
        tokio::spawn(usb::run(config.port, shutdown.clone()));
    }
    {
        // Reconnect touch if KWin restarts.
        let touch = touch.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let mut t = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! { _ = shutdown.cancelled() => break, _ = t.tick() => touch.ensure().await }
            }
        });
    }

    info!(name = %server_name, port = config.port, fingerprint = %identity.short_fingerprint(), "slated ready");
    let serve = tokio::spawn(server::serve(listener, acceptor, ctx, shutdown.clone()));

    wait_for_signal().await;
    info!("shutting down");
    registry.cancel_all();
    shutdown.cancel();
    for _ in 0..40 {
        if registry.len() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = serve.await;
    Ok(())
}

async fn wait_for_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            error!("signal handler: {e}");
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

async fn doctor() -> anyhow::Result<()> {
    let mut ok = true;
    let mut check = |name: &str, pass: bool, hint: &str| {
        println!("[{}] {name}{}", if pass { " ok " } else { "FAIL" }, if pass { String::new() } else { format!(" — {hint}") });
        ok &= pass;
    };
    check(
        "Wayland session",
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        "run inside a Plasma Wayland session (or source scripts/dev-env.sh)",
    );
    let vo = slate_kwin::virtual_output::VirtualOutputManager::connect();
    check(
        "KWin virtual outputs (zkde_screencast_unstable_v1)",
        vo.is_ok(),
        "install packaging/io.github.hassanm0301.plasmaslate.desktop with Exec= pointing at this binary",
    );
    let touch = slate_kwin::eis::TouchInjector::connect().await;
    check("KWin EIS touch injection", touch.as_ref().map(|t| t.is_ready()).unwrap_or(false), "same .desktop file (X-KDE-DBUS-Restricted-Interfaces)");
    let encs = slate_media::available_encoders();
    check(
        &format!("hardware encoder ({})", encs.join(", ")),
        encs.iter().any(|e| e.starts_with("va")),
        "install gst-plugin-va (falls back to x264 software encoding)",
    );
    let kd = tokio::process::Command::new("kscreen-doctor").arg("-j").output().await;
    check("kscreen-doctor", kd.map(|o| o.status.success()).unwrap_or(false), "install libkscreen");
    let adb = tokio::process::Command::new("adb").arg("version").output().await.is_ok();
    check("adb (USB connections, optional)", adb, "install android-tools to connect over USB");
    if ok {
        println!("\nAll checks passed.");
        Ok(())
    } else {
        anyhow::bail!("some checks failed")
    }
}
