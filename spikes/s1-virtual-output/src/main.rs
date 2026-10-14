//! Spike S1: create a KWin virtual output via zkde_screencast_unstable_v1.
//!
//! Usage: s1-virtual-output [NAME] [WIDTH] [HEIGHT] [SCALE] [SECONDS]
//! WIDTH/HEIGHT are *logical* sizes (physical = logical * scale).
//! Keeps the stream (and so the output) alive for SECONDS, printing every
//! protocol event, then closes it cleanly.

use std::time::{Duration, Instant};

use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle};

#[allow(dead_code, non_camel_case_types, non_upper_case_globals, clippy::all)]
pub mod screencast {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("../../protocols/zkde-screencast-unstable-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("../../protocols/zkde-screencast-unstable-v1.xml");
}

use screencast::zkde_screencast_stream_unstable_v1::{self as stream, ZkdeScreencastStreamUnstableV1};
use screencast::zkde_screencast_unstable_v1::{self as sc, ZkdeScreencastUnstableV1};

#[derive(Default)]
struct State {
    screencast: Option<(u32, u32)>, // (global name, version)
    outputs: Vec<String>,
    node: Option<u32>,
    failed: Option<String>,
    closed: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version } => {
                if interface == "zkde_screencast_unstable_v1" {
                    state.screencast = Some((name, version));
                } else if interface == "wl_output" {
                    registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
                }
            }
            wl_registry::Event::GlobalRemove { name } => {
                println!("[registry] global removed: {name}");
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Name { name } => {
                println!("[wl_output] name={name}");
                state.outputs.push(name);
            }
            wl_output::Event::Description { description } => println!("[wl_output]   description={description}"),
            wl_output::Event::Mode { width, height, refresh, .. } => {
                println!("[wl_output]   mode={width}x{height}@{}", f64::from(refresh) / 1000.0)
            }
            wl_output::Event::Geometry { x, y, make, model, .. } => {
                println!("[wl_output]   geometry pos={x},{y} make={make} model={model}")
            }
            wl_output::Event::Scale { factor } => println!("[wl_output]   scale={factor}"),
            _ => {}
        }
    }
}

impl Dispatch<ZkdeScreencastUnstableV1, ()> for State {
    fn event(_: &mut Self, _: &ZkdeScreencastUnstableV1, _: sc::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZkdeScreencastStreamUnstableV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZkdeScreencastStreamUnstableV1,
        event: stream::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            stream::Event::Created { node } => {
                println!("[stream] created: pipewire node={node}");
                state.node = Some(node);
            }
            stream::Event::Serial { object_serial_hi, object_serial_low } => {
                let serial = (u64::from(object_serial_hi) << 32) | u64::from(object_serial_low);
                println!("[stream] pipewire object serial={serial}");
            }
            stream::Event::Failed { error } => {
                println!("[stream] FAILED: {error}");
                state.failed = Some(error);
            }
            stream::Event::Closed => {
                println!("[stream] closed by compositor");
                state.closed = true;
            }
            _ => {}
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = args.get(1).cloned().unwrap_or_else(|| "PlasmaSlate-spike".into());
    let width: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1280);
    let height: i32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(800);
    let scale: f64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1.5);
    let seconds: u64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(20);

    let conn = Connection::connect_to_env().expect("connect to Wayland (is WAYLAND_DISPLAY set?)");
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let display = conn.display();
    let registry = display.get_registry(&qh, ());

    let mut state = State::default();
    queue.roundtrip(&mut state).expect("roundtrip");
    queue.roundtrip(&mut state).expect("roundtrip");
    println!("outputs before: {:?}", state.outputs);

    let Some((global, version)) = state.screencast else {
        eprintln!("zkde_screencast_unstable_v1 not advertised to this client.");
        eprintln!("KWin restricts it: install a .desktop file whose Exec= is this binary's absolute path");
        eprintln!("with X-KDE-Wayland-Interfaces=zkde_screencast_unstable_v1 (see scripts/install-dev-desktop-files.sh).");
        std::process::exit(2);
    };
    println!("zkde_screencast_unstable_v1 version {version}");
    let screencast: ZkdeScreencastUnstableV1 = registry.bind(global, version.min(6), &qh, ());

    let pointer = sc::Pointer::Embedded;
    let stream = if version >= 4 {
        screencast.stream_virtual_output_with_description(
            name.clone(),
            format!("Plasma Slate ({name})"),
            width,
            height,
            scale,
            pointer.into(),
            &qh,
            (),
        )
    } else {
        screencast.stream_virtual_output(name.clone(), width, height, scale, pointer.into(), &qh, ())
    };
    println!("requested virtual output name={name} logical={width}x{height} scale={scale}");

    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline && !state.closed && state.failed.is_none() {
        queue.roundtrip(&mut state).expect("roundtrip");
        std::thread::sleep(Duration::from_millis(250));
    }

    // Re-scan outputs so we can see the virtual one listed.
    state.outputs.clear();
    let registry2 = display.get_registry(&qh, ());
    queue.roundtrip(&mut state).expect("roundtrip");
    queue.roundtrip(&mut state).expect("roundtrip");
    drop(registry2);
    println!("outputs at end: {:?}", state.outputs);

    stream.close();
    queue.roundtrip(&mut state).expect("roundtrip");
    println!("closed stream; node={:?}", state.node);
}
