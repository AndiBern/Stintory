// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A stand-in for Le Mans Ultimate behind another shared-memory bridge on
//! Linux: writes `/dev/shm/LMU_Data` the way the game writes into it when
//! simshmbridge or DataLink backs its mapping with that file, and listens on
//! the game's REST port (127.0.0.1:6397) so the app sees a running game.
//! Lets the app's `/dev/shm` reader be tried without the game:
//!
//! ```sh
//! cargo run -p sy-source-lmu --example fake_lmu_shm -- [laps] [time-scale] [path]
//! ```
//!
//! It drives an out-lap and `laps` laps (default 2) `time-scale` times
//! faster than real time (default 4), goes back to the garage for 3 s and
//! exits. Like simshmbridge's `lmushm`, it leaves the file behind (505,000
//! bytes, the size simshmbridge uses); remove it with `rm /dev/shm/LMU_Data`.

#[cfg(not(unix))]
fn main() {
    eprintln!("fake_lmu_shm is for Linux; on Windows use sy-bridge's fake_lmu example");
    std::process::exit(2);
}

#[cfg(unix)]
fn main() {
    use std::fs::OpenOptions;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::os::unix::fs::FileExt;
    use std::time::{Duration, Instant};

    use sy_source_lmu::layout::{
        OBJECT_SIZE, OFF_GAME_VERSION, OFF_SCORING_INFO, OFF_TELEM_INFO, OFF_TELEMETRY,
        OFF_VEH_SCORING, SCORING_INFO_SIZE, VEH_SCORING_SIZE,
    };
    use sy_source_lmu::sim::SimLmu;

    /// simshmbridge's `LMU_SIZE`
    const FILE_SIZE: u64 = 505_000;
    const _: () = assert!(OBJECT_SIZE as u64 <= FILE_SIZE);

    let mut args = std::env::args().skip(1);
    let laps: i16 = args.next().map_or(2, |a| a.parse().expect("laps"));
    let scale: f64 = args.next().map_or(4.0, |a| a.parse().expect("time scale"));
    let path = args
        .next()
        .unwrap_or_else(|| sy_source_lmu::live::SHM_PATH.into());

    // the game's REST API: answer every request with 404 (no car setup)
    let rest = TcpListener::bind("127.0.0.1:6397").expect("port 6397 is free");
    std::thread::spawn(move || {
        for mut stream in rest.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        }
    });

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .expect("open the file");
    if file.metadata().expect("metadata").len() < FILE_SIZE {
        file.set_len(FILE_SIZE).expect("size the file");
    }
    let write = |at: usize, bytes: &[u8]| file.write_all_at(bytes, at as u64).expect("write");
    println!("fake LMU: writing {path}, REST on 127.0.0.1:6397, {laps} laps at {scale}x");

    let mut sim = SimLmu::new();
    let dt = 0.01;
    let mut next_scoring = sim.et;
    let mut garage_until: Option<Instant> = None;
    let mut frames = 0u64;
    loop {
        if garage_until.is_none() && sim.total_laps > laps {
            println!("fake LMU: back to the garage");
            sim.in_realtime = false;
            garage_until = Some(Instant::now() + Duration::from_secs(3));
        }
        if garage_until.is_some_and(|t| Instant::now() >= t) {
            break;
        }
        let _ = sim.drive(dt);
        write(OFF_GAME_VERSION, &1_200i32.to_le_bytes());
        if sim.et + 1e-9 >= next_scoring {
            next_scoring += 0.2;
            let rec = sim.scoring();
            let p = &rec.payload[12..];
            write(OFF_SCORING_INFO, &p[..SCORING_INFO_SIZE]);
            // the player is the first of 20 cars
            write(OFF_VEH_SCORING, &p[SCORING_INFO_SIZE..][..VEH_SCORING_SIZE]);
        }
        if sim.in_realtime {
            // numVehicles active, playerVehicleIdx 0, playerHasVehicle
            write(OFF_TELEMETRY, &[1, 0, 1]);
            write(OFF_TELEM_INFO, &sim.telemetry());
            frames += 1;
        } else {
            write(OFF_TELEMETRY, &[1, 0, 0]);
        }
        std::thread::sleep(Duration::from_secs_f64(dt / scale));
    }
    println!("fake LMU: exiting after {frames} telemetry frames; {path} stays");
}
