// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sends synthetic F1 25 packets over UDP, like the game would: a time
//! trial on an oval with an out-lap, a clean lap, an invalid lap and a lap
//! with a flashback. For trying the desktop app without the game.
//!
//! ```sh
//! cargo run -p sy-source-f1 --example simulate -- [--to 127.0.0.1:20777] [--format 2025|2026] [--rate 60] [--speedup 4]
//! ```

use std::net::UdpSocket;
use std::time::Duration;

use sy_source_f1::packets::Format;
use sy_source_f1::sim::SimGame;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut to = "127.0.0.1:20777".to_owned();
    let mut format = Format::F2026;
    let mut rate = 60.0f32;
    let mut speedup = 1.0f32;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--to" => to = value()?,
            "--format" => {
                format = Format::from_packet_format(value()?.parse()?)
                    .ok_or("format must be 2025 or 2026")?;
            }
            "--rate" => rate = value()?.parse()?,
            "--speedup" => speedup = value()?.parse()?,
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let dt = 1.0 / rate;
    let mut g = SimGame::new(format);
    g.uid = u64::from(std::process::id()) << 16 | 0xF1;
    g.track_length = 2000;
    let send = |packets: &[Vec<u8>]| -> std::io::Result<()> {
        for p in packets {
            socket.send_to(p, &to)?;
        }
        Ok(())
    };
    let tick = |g: &mut SimGame, frame: usize| -> std::io::Result<()> {
        g.advance(dt);
        send(&g.frame())?;
        if frame.is_multiple_of((rate as usize / 2).max(1)) {
            send(&[g.session()])?;
        }
        if frame.is_multiple_of((rate as usize * 5).max(1)) {
            send(&[g.participants(), g.session_history()])?;
        }
        std::thread::sleep(Duration::from_secs_f32(dt / speedup));
        Ok(())
    };
    println!("sending {format} packets to {to} at {rate} Hz (x{speedup})");
    send(&[g.session(), g.participants(), g.event(b"SSTA", &[])])?;
    let mut frame = 0;
    let mut drive_lap = |g: &mut SimGame, invalid_at: Option<f32>| -> std::io::Result<()> {
        let lap = g.lap;
        while g.lap == lap {
            if invalid_at.is_some_and(|d| g.dist > d) {
                g.invalid = true;
            }
            frame += 1;
            tick(g, frame)?;
        }
        send(&[g.session_history()])?;
        println!("lap {lap} done");
        Ok(())
    };
    // out-lap from the pit lane
    g.dist = 1200.0;
    g.pit = 2;
    g.driver_status = 3;
    for _ in 0..rate as usize {
        tick(&mut g, 0)?;
    }
    g.pit = 0;
    drive_lap(&mut g, None)?;
    g.driver_status = 1;
    g.speed_kmh = 200;
    drive_lap(&mut g, None)?;
    drive_lap(&mut g, Some(800.0))?;
    // flashback halfway round
    while g.dist < 1000.0 {
        tick(&mut g, 1)?;
    }
    let back = g.clone();
    while g.dist < 1400.0 {
        tick(&mut g, 1)?;
    }
    send(&[g.flashback(back.time)])?;
    g = back;
    drive_lap(&mut g, None)?;
    send(&[g.event(b"SEND", &[])])?;
    println!("session ended");
    Ok(())
}
