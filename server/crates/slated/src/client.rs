//! Developer test client: behaves like the Android app (handshake, pairing, receive video,
//! send touch) so the daemon can be exercised end-to-end from a terminal or CI.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use rustls_pki::ServerName;
use slate_net::tls::client::{connector, PinnedVerifier};
use slate_protocol::framing::{read_frame, write_frame};
use slate_protocol::pb;
use slate_protocol::{Envelope, Msg, PROTOCOL_VERSION};
use tokio::io::AsyncWriteExt;
use tracing::info;

mod rustls_pki {
    pub use tokio_rustls::rustls::pki_types::ServerName;
}

/// Options for the test client.
#[derive(Debug, Clone, clap::Args)]
pub struct ClientArgs {
    /// Server address.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    #[arg(long, default_value_t = slate_protocol::DEFAULT_PORT)]
    pub port: u16,
    /// Device id to present (stable across runs to stay paired).
    #[arg(long, default_value = "devclient-0000-0000-0000-000000000001")]
    pub device_id: String,
    #[arg(long, default_value = "slated test client")]
    pub device_name: String,
    /// Panel size the client claims.
    #[arg(long, default_value_t = 1920)]
    pub width: u32,
    #[arg(long, default_value_t = 1200)]
    pub height: u32,
    /// Stop after this many video frames.
    #[arg(long, default_value_t = 120)]
    pub frames: u64,
    /// Write the received Annex-B stream to this file.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// After receiving video, send a tap at this normalized position (x,y).
    #[arg(long, value_parser = parse_pair)]
    pub tap: Option<(f32, f32)>,
    /// After receiving video, drag from tap position by this normalized delta (dx,dy).
    #[arg(long, value_parser = parse_pair)]
    pub drag: Option<(f32, f32)>,
    /// File storing the auth token + pinned fingerprint between runs.
    #[arg(long)]
    pub state: Option<PathBuf>,
}

fn parse_pair(s: &str) -> Result<(f32, f32), String> {
    let (a, b) = s.split_once(',').ok_or("expected x,y")?;
    Ok((
        a.trim().parse().map_err(|e| format!("{e}"))?,
        b.trim().parse().map_err(|e| format!("{e}"))?,
    ))
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct ClientState {
    token: String,
    fingerprint: Option<String>,
}

pub async fn run(args: ClientArgs) -> anyhow::Result<()> {
    let state_path = args
        .state
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("slated-test-client.json"));
    let mut state: ClientState = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let verifier = PinnedVerifier::new(state.fingerprint.clone());
    let tcp = tokio::net::TcpStream::connect((args.host.as_str(), args.port))
        .await
        .context("connect")?;
    tcp.set_nodelay(true)?;
    let mut tls = connector(verifier.clone())?
        .connect(ServerName::try_from("plasma-slate.local")?, tcp)
        .await
        .context("TLS")?;
    info!(fingerprint = ?verifier.seen(), "TLS established");

    let hello = pb::ClientHello {
        protocol_version: PROTOCOL_VERSION,
        device_id: args.device_id.clone(),
        device_name: args.device_name.clone(),
        app_version: format!("slated-client {}", env!("CARGO_PKG_VERSION")),
        display: Some(pb::DisplayInfo {
            width_px: args.width,
            height_px: args.height,
            physical_width_mm: 217.0,
            physical_height_mm: 136.0,
            density_dpi: 224,
            refresh_hz: 60.0,
            orientation: pb::Orientation::Landscape as i32,
        }),
        touch: Some(pb::TouchInfo {
            supported: true,
            max_points: 10,
        }),
        decoders: vec![pb::DecoderInfo {
            codec: pb::Codec::H264 as i32,
            max_width: 4096,
            max_height: 2304,
            max_fps: 60,
            hardware: false,
            name: "file".into(),
        }],
        auth_token: state.token.clone(),
        link: pb::Link::Network as i32,
    };
    write_frame(&mut tls, &Envelope::new(Msg::ClientHello(hello))).await?;

    let server_hello = loop {
        let env = tokio::time::timeout(Duration::from_secs(90), read_frame(&mut tls))
            .await
            .context("waiting for server")??;
        match env.msg {
            Some(Msg::PairingPending(p)) => {
                info!(fingerprint = %p.server_fingerprint, "{}", p.message)
            }
            Some(Msg::ServerHello(h)) => break h,
            Some(Msg::Error(e)) => bail!("server error {}: {}", e.code, e.message),
            other => bail!("unexpected {:?}", other.map(|m| Envelope::new(m).kind())),
        }
    };
    if !server_hello.issued_auth_token.is_empty() {
        state.token = server_hello.issued_auth_token.clone();
        state.fingerprint = verifier.seen();
        std::fs::write(&state_path, serde_json::to_string(&state)?)?;
        info!(path = %state_path.display(), "paired; token stored");
    }
    let stream = server_hello.stream.unwrap_or_default();
    info!(output = %server_hello.output_name, w = stream.width, h = stream.height, kbps = stream.bitrate_kbps, "session started");
    println!("OUTPUT {}", server_hello.output_name);
    write_frame(&mut tls, &Envelope::new(Msg::Ready(pb::Ready {}))).await?;

    let mut out = match &args.out {
        Some(p) => Some(tokio::fs::File::create(p).await?),
        None => None,
    };
    let start = Instant::now();
    let (mut frames, mut bytes, mut keyframes) = (0u64, 0u64, 0u64);
    let mut touched = false;
    while frames < args.frames {
        let env = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut tls))
            .await
            .context("waiting for video")??;
        match env.msg {
            Some(Msg::VideoFrame(f)) => {
                frames += 1;
                bytes += f.data.len() as u64;
                keyframes += u64::from(f.keyframe);
                if let Some(o) = out.as_mut() {
                    o.write_all(&f.data).await?;
                }
                if frames == 10 && !touched {
                    touched = true;
                    send_touch(&mut tls, &args).await?;
                }
            }
            Some(Msg::Ping(p)) => {
                write_frame(
                    &mut tls,
                    &Envelope::new(Msg::Pong(pb::Pong {
                        id: p.id,
                        ping_sender_time_us: p.sender_time_us,
                        responder_time_us: 0,
                    })),
                )
                .await?;
            }
            Some(Msg::Error(e)) => bail!("server error {}: {}", e.code, e.message),
            Some(Msg::Goodbye(g)) => bail!("server goodbye: {}", g.reason),
            _ => {}
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "RECEIVED frames={frames} keyframes={keyframes} bytes={bytes} fps={:.1} kbps={:.0}",
        frames as f64 / secs,
        bytes as f64 * 8.0 / secs / 1000.0
    );
    write_frame(
        &mut tls,
        &Envelope::new(Msg::Goodbye(pb::Goodbye {
            reason: "test done".into(),
        })),
    )
    .await?;
    let _ = tls.shutdown().await;
    Ok(())
}

async fn send_touch<S: tokio::io::AsyncWrite + Unpin>(
    s: &mut S,
    args: &ClientArgs,
) -> anyhow::Result<()> {
    let Some((x, y)) = args.tap else {
        return Ok(());
    };
    let ev = |phase: pb::TouchPhase, x: f32, y: f32| {
        Envelope::new(Msg::TouchEvent(pb::TouchEvent {
            time_us: 0,
            points: vec![pb::TouchPoint {
                id: 0,
                phase: phase as i32,
                x,
                y,
            }],
        }))
    };
    write_frame(s, &ev(pb::TouchPhase::Down, x, y)).await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    if let Some((dx, dy)) = args.drag {
        for i in 1..=30 {
            let f = i as f32 / 30.0;
            write_frame(s, &ev(pb::TouchPhase::Move, x + dx * f, y + dy * f)).await?;
            tokio::time::sleep(Duration::from_millis(16)).await;
        }
        write_frame(s, &ev(pb::TouchPhase::Up, x + dx, y + dy)).await?;
    } else {
        write_frame(s, &ev(pb::TouchPhase::Up, x, y)).await?;
    }
    info!("touch sent");
    Ok(())
}
