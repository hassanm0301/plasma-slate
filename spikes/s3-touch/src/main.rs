//! Spike S3: inject touch through KWin's EIS endpoint.
//!
//! Usage:
//!   s3-touch caps <N>                     connect with capability mask N and list devices/regions
//!   s3-touch tap  <x> <y>                 tap at global logical coordinates
//!   s3-touch drag <x> <y> <dx> <dy> [ms]  drag from (x,y) by (dx,dy)
//!   s3-touch pinch <cx> <cy> <r0> <r1>    two-finger pinch around (cx,cy)

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use reis::ei;
use reis::event::{DeviceCapability, EiEvent};

fn connect(caps: i32) -> (ei::Context, i32) {
    let conn = zbus::blocking::Connection::session().expect("session bus");
    let reply = conn
        .call_method(
            Some("org.kde.KWin"),
            "/org/kde/KWin/EIS/RemoteDesktop",
            Some("org.kde.KWin.EIS.RemoteDesktop"),
            "connectToEIS",
            &(caps,),
        )
        .expect("connectToEIS (is the restricted-interface .desktop file installed?)");
    let (fd, cookie): (zbus::zvariant::OwnedFd, i32) = reply.body().deserialize().expect("reply");
    let fd: OwnedFd = fd.into();
    let ctx = ei::Context::new(UnixStream::from(fd)).expect("ei context");
    // Keep the D-Bus connection alive for the lifetime of the process: KWin may
    // tie the EIS client to the caller.
    std::mem::forget(conn);
    (ctx, cookie)
}

struct Touch {
    ctx: ei::Context,
    device: reis::event::Device,
    serial: u32,
    start: Instant,
}

impl Touch {
    fn ts(&self) -> &ei::Touchscreen {
        // Interface proxies are cheap clones.
        Box::leak(Box::new(self.device.interface::<ei::Touchscreen>().expect("touch iface")))
    }
    fn frame(&self) {
        let us = self.start.elapsed().as_micros() as u64;
        self.device.device().frame(self.serial, us);
        let _ = self.ctx.flush();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("caps");
    let num = |i: usize, d: f32| args.get(i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(d);
    let caps: i32 = if cmd == "caps" { num(2, 4.0) as i32 } else { 4 };

    let (ctx, cookie) = connect(caps);
    println!("connected, cookie={cookie}");
    let (_connection, mut events) = ctx.handshake_blocking("plasma-slate-s3", ei::handshake::ContextType::Sender).expect("handshake");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut touch: Option<Touch> = None;
    for ev in events.by_ref() {
        let ev = ev.expect("ei event");
        match &ev {
            EiEvent::SeatAdded(s) => {
                println!("seat: {:?}", s.seat.name());
                s.seat.bind_capabilities(
                    DeviceCapability::Touch | DeviceCapability::PointerAbsolute | DeviceCapability::Pointer | DeviceCapability::Keyboard,
                );
                let _ = ctx.flush();
            }
            EiEvent::DeviceAdded(d) => {
                let dev = &d.device;
                println!(
                    "device: {:?} type={:?} touch={} abs={} regions={:?}",
                    dev.name(),
                    dev.device_type(),
                    dev.has_capability(DeviceCapability::Touch),
                    dev.has_capability(DeviceCapability::PointerAbsolute),
                    dev.regions()
                );
            }
            EiEvent::DeviceResumed(r) => {
                println!("resumed: {:?} serial={}", r.device.name(), r.serial);
                if r.device.has_capability(DeviceCapability::Touch) && touch.is_none() {
                    r.device.device().start_emulating(r.serial, 1);
                    touch = Some(Touch { ctx: ctx.clone(), device: r.device.clone(), serial: r.serial, start: Instant::now() });
                }
            }
            other => println!("event: {other:?}"),
        }
        if touch.is_some() && cmd != "caps" {
            break;
        }
        if Instant::now() > deadline {
            break;
        }
    }

    let Some(t) = touch else {
        println!("no touch device resumed");
        return;
    };
    let ts = t.ts();
    let step = Duration::from_millis(16);
    match cmd {
        "tap" => {
            let (x, y) = (num(2, 0.0), num(3, 0.0));
            ts.down(1, x, y);
            t.frame();
            std::thread::sleep(Duration::from_millis(60));
            ts.up(1);
            t.frame();
            println!("tapped at {x},{y}");
        }
        "drag" => {
            let (x, y, dx, dy, ms) = (num(2, 0.0), num(3, 0.0), num(4, 0.0), num(5, 0.0), num(6, 600.0));
            let steps = (ms / 16.0).max(2.0) as u32;
            ts.down(1, x, y);
            t.frame();
            std::thread::sleep(Duration::from_millis(150));
            for i in 1..=steps {
                let f = i as f32 / steps as f32;
                ts.motion(1, x + dx * f, y + dy * f);
                t.frame();
                std::thread::sleep(step);
            }
            ts.up(1);
            t.frame();
            println!("dragged from {x},{y} by {dx},{dy}");
        }
        "pinch" => {
            let (cx, cy, r0, r1) = (num(2, 0.0), num(3, 0.0), num(4, 50.0), num(5, 200.0));
            ts.down(1, cx - r0, cy);
            ts.down(2, cx + r0, cy);
            t.frame();
            for i in 1..=30 {
                let r = r0 + (r1 - r0) * i as f32 / 30.0;
                ts.motion(1, cx - r, cy);
                ts.motion(2, cx + r, cy);
                t.frame();
                std::thread::sleep(step);
            }
            ts.up(1);
            ts.up(2);
            t.frame();
            println!("pinched");
        }
        _ => {}
    }
    t.device.device().stop_emulating(t.serial);
    let _ = ctx.flush();
    std::thread::sleep(Duration::from_millis(100));
}
