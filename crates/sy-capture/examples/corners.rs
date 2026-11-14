// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Prints the corner analysis of a recording (development: tuning the
//! corner detection on real laps).
//!
//! Records the given `.ibt` or raw `.syraw` files and compares, per
//! session, the second fastest valid lap with the fastest one (or the laps
//! with the given numbers).
//!
//! ```sh
//! cargo run --release -p sy-capture --example corners -- <file>... [--laps <ref>,<lap>]
//! ```
//!
//! `--all` instead compares every lap with the fastest one and only prints
//! a summary per lap (robustness check).

use std::path::PathBuf;
use std::time::Duration;

use sy_capture::pipeline::{SpoolChain, decoder_for, recorder_config};
use sy_capture::{RawSource, RecordedLap, Recorder, RecorderOutput};
use sy_model::api::{CornerLapStats, TurnDirection};
use sy_model::{BlobHash, GameId, Lap, Session};
use sy_source_iracing::IbtSource;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    let mut pick: Option<(u32, u32)> = None;
    let mut all = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--all" {
            all = true;
        } else if a == "--laps" {
            let v = args.next().ok_or("--laps <ref>,<lap>")?;
            let (a, b) = v.split_once(',').ok_or("--laps <ref>,<lap>")?;
            pick = Some((a.parse()?, b.parse()?));
        } else {
            files.push(PathBuf::from(a));
        }
    }
    if files.is_empty() {
        eprintln!("usage: corners <file>... [--laps <ref>,<lap>]");
        std::process::exit(2);
    }

    let mut source: Box<dyn RawSource> = if files[0].extension().is_some_and(|e| e == "ibt") {
        Box::new(IbtSource::open_many(&files)?)
    } else {
        Box::new(SpoolChain::open(&files)?)
    };
    let (sessions, laps) = record(source.as_mut())?;

    for s in &sessions {
        let mut own: Vec<&RecordedLap> = laps
            .iter()
            .filter(|l| l.lap.session_id == s.id && l.lap.lap_time_ms.is_some())
            .collect();
        println!(
            "\n=== {} / {} ({} laps)",
            s.track_name,
            s.car_name,
            own.len()
        );
        if all {
            let Some(best) = own
                .iter()
                .filter(|l| l.lap.valid && !l.lap.flags.out_lap && !l.lap.flags.in_lap)
                .min_by_key(|l| l.lap.lap_time_ms)
            else {
                continue;
            };
            let best_meta = meta(best);
            for l in &own {
                let lm = meta(l);
                let r =
                    sy_analysis::corners(&[(&best_meta, &best.telemetry), (&lm, &l.telemetry)])?;
                let lost: Vec<Option<f32>> =
                    r.laps[1].corners.iter().map(|c| c.time_lost_s).collect();
                let sum: f32 = lost.iter().flatten().sum();
                println!(
                    "  lap {:>2} {:>8.3} valid={:<5} out={:<5} in={:<5} corners={:>2} measured={:>2} lost {:+.3} vs {:+.3}",
                    l.lap.number,
                    secs(l.lap.lap_time_ms),
                    l.lap.valid,
                    l.lap.flags.out_lap,
                    l.lap.flags.in_lap,
                    r.corners.len(),
                    lost.iter().flatten().count(),
                    sum,
                    secs(l.lap.lap_time_ms) - secs(best.lap.lap_time_ms)
                );
            }
            continue;
        }
        let (reference, lap) = match pick {
            Some((a, b)) => {
                let find = |n| own.iter().find(|l| l.lap.number == n).copied();
                match (find(a), find(b)) {
                    (Some(a), Some(b)) => (a, b),
                    _ => continue,
                }
            }
            None => {
                own.retain(|l| l.lap.valid && !l.lap.flags.out_lap && !l.lap.flags.in_lap);
                own.sort_by_key(|l| l.lap.lap_time_ms);
                match own.as_slice() {
                    [a, b, ..] => (*a, *b),
                    [a] => (*a, *a),
                    [] => continue,
                }
            }
        };
        let (ref_lap, cmp_lap) = (meta(reference), meta(lap));
        let report =
            sy_analysis::corners(&[(&ref_lap, &reference.telemetry), (&cmp_lap, &lap.telemetry)])?;
        println!(
            "reference lap {} {:.3} s, lap {} {:.3} s",
            reference.lap.number,
            secs(reference.lap.lap_time_ms),
            lap.lap.number,
            secs(lap.lap.lap_time_ms)
        );
        println!(
            "  #  dir    seg      start  apex   end   R(m) | {:>6} | {:>13} {:>11} {:>11} {:>11} {:>11} {:>11} {:>11} {:>9} | lost (entry/exit)",
            "time", "brake@ (max)", "release@", "lift@", "vmin@", "thr@", "full@", "exit", "top"
        );
        let mut total = 0.0;
        for (i, c) in report.corners.iter().enumerate() {
            let rs = &report.laps[0].corners[i];
            let ls = &report.laps[1].corners[i];
            let dir = match c.direction {
                TurnDirection::Left => "L",
                TurnDirection::Right => "R",
            };
            println!(
                "{:>3}  {}  {:>5}-{:<5} {:>5} {:>5} {:>5} {:>5.0} | {}",
                c.number,
                dir,
                c.segment_start_m,
                c.segment_end_m,
                c.start_m,
                c.apex_m,
                c.end_m,
                c.min_radius_m,
                stats(rs),
            );
            println!("{:>48} | {}", "", stats(ls));
            total += ls.time_lost_s.unwrap_or(0.0);
        }
        println!(
            "sum of time lost {total:+.3} s, lap difference {:+.3} s",
            secs(lap.lap.lap_time_ms) - secs(reference.lap.lap_time_ms)
        );
    }
    Ok(())
}

fn stats(s: &CornerLapStats) -> String {
    let m = |v: Option<f32>| v.map_or("-".into(), |v| format!("{v:.0}"));
    let kmh = |v: Option<f32>| v.map_or("-".into(), |v| format!("{:.0}", v * 3.6));
    format!(
        "{:>6} | {:>6} ({:>4}) {:>11} {:>11} {:>5}@{:<5} {:>11} {:>11} {:>9} {:>9} | {}{}",
        s.time_s.map_or("-".into(), |t| format!("{t:.3}")),
        m(s.brake_m),
        s.brake_max
            .map_or("-".into(), |b| format!("{:.0}%", b * 100.0)),
        m(s.brake_release_m),
        m(s.lift_m),
        kmh(s.min_speed_mps),
        m(s.min_speed_m),
        m(s.throttle_m),
        m(s.full_throttle_m),
        kmh(s.exit_speed_mps),
        kmh(s.top_speed_mps),
        s.time_lost_s.map_or("-".into(), |t| format!(
            "{t:+.3} ({:+.3}/{:+.3})",
            s.entry_lost_s.unwrap_or(f32::NAN),
            s.exit_lost_s.unwrap_or(f32::NAN)
        )),
        if s.flat_out { " flat" } else { "" },
    )
}

fn secs(ms: Option<u32>) -> f32 {
    ms.map_or(f32::NAN, |t| t as f32 / 1000.0)
}

fn meta(l: &RecordedLap) -> Lap {
    l.lap
        .clone()
        .into_lap(BlobHash::of(b""), l.telemetry.sample_rate_hz(), 0)
}

type Recorded = (Vec<Session>, Vec<RecordedLap>);

fn record(source: &mut dyn RawSource) -> Result<Recorded, Box<dyn std::error::Error>> {
    let game: GameId = source.game();
    let mut decoder = decoder_for(game).ok_or("no decoder for this game")?;
    let mut recorder = Recorder::new(recorder_config(game));
    let mut out = Vec::new();
    while let Some(rec) = source.next_record(Duration::ZERO)? {
        for event in decoder.decode(&rec)? {
            out.extend(recorder.handle(event));
        }
    }
    for event in decoder.finish() {
        out.extend(recorder.handle(event));
    }
    out.extend(recorder.finish());
    let mut sessions: Vec<Session> = Vec::new();
    let mut laps = Vec::new();
    for o in out {
        match o {
            RecorderOutput::SessionStarted(s)
            | RecorderOutput::SessionUpdated(s)
            | RecorderOutput::SessionEnded(s) => match sessions.iter_mut().find(|x| x.id == s.id) {
                Some(x) => *x = s,
                None => sessions.push(s),
            },
            RecorderOutput::Lap(l) => laps.push(*l),
        }
    }
    Ok((sessions, laps))
}
