// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Corner insights (docs/ARCHITECTURE.md §6.5): finds the corners of a track on the
//! reference lap and measures how each lap drives them: braking point, lowest
//! speed, throttle pick-up, exit speed and the time won or lost.
//!
//! - Corners come from the curvature of the reference lap's line,
//!   `g_lat / speed²` (1/radius), smoothed over a few metres. Every game has
//!   lateral acceleration with the same sign, unlike positions (each game has
//!   its own axes) or the steering angle (depends on car and driver).
//! - Every lap is measured at the same distances, so the corners are the
//!   same for all laps.
//! - The segments (see [`Corner`]) cover the whole lap: their times add up to
//!   the lap time, so the corners where time is lost explain the gap.

use std::ops::Range;

use sy_model::api::{Corner, CornerLapStats, CornerReport, LapCorners, TurnDirection};
use sy_model::channel::names;
use sy_model::{Lap, LapTelemetry};

use crate::{AnalysisError, Resampled, common_grid_len, resample};

/// Grid resolution of the corner analysis, metres.
const STEP_M: f32 = 1.0;
/// Half width of the curvature smoothing window, grid points.
const SMOOTH_HALF: usize = 10;
/// Speeds below this don't give a useful curvature (pit lane, spins), m/s.
const MIN_CURVATURE_SPEED: f32 = 10.0;
/// A corner needs a radius below this somewhere, metres.
const CORNER_RADIUS_M: f32 = 250.0;
/// A corner extends while the radius is below this, metres.
const CORNER_EDGE_RADIUS_M: f32 = 500.0;
/// Corners in the same direction closer than this are one (double apex).
const MERGE_GAP_M: f32 = 30.0;
/// Shorter bends are ignored (kerb hits, a twitch of the car), metres.
const MIN_CORNER_LEN_M: f32 = 15.0;
/// The segment starts this far before braking or turning in, metres.
const APPROACH_MARGIN_M: f32 = 10.0;
/// Gaps in braking up to this long still count as one braking, metres.
const BRAKE_GAP_M: f32 = 5.0;
/// The lowest speed in a corner is its apex only if the speed is higher
/// everywhere this close around it, metres. Otherwise the speed still falls
/// into the next corner (or rises from the previous one), and the tightest
/// point is the apex.
const LOCAL_MIN_M: f32 = 30.0;
/// A bend that starts less than this after the braking for the next corner
/// is no corner of its own but part of that braking zone, metres.
const BRAKING_BEND_M: f32 = 50.0;
/// A lap whose data starts this close to the line is a full lap, metres.
const LAP_START_M: f32 = 20.0;

/// Brake pressure that counts as braking.
const BRAKE_ON: f32 = 0.05;
/// Throttle below this is a lift.
const LIFT: f32 = 0.9;
/// Throttle at or above this is full throttle.
const FULL: f32 = 0.95;
/// Throttle at or above this counts as being on the throttle.
const THROTTLE_ON: f32 = 0.1;

const CHANNELS: &[&str] = &[
    names::TIME,
    names::SPEED,
    names::THROTTLE,
    names::BRAKE,
    names::GEAR,
    names::G_LAT,
];

/// A corner on the grid (indices = metres from the start/finish line).
#[derive(Debug, Clone)]
struct Region {
    direction: TurnDirection,
    start: usize,
    /// exclusive
    end: usize,
    /// where the reference is slowest, or the tightest point if the speed
    /// has no minimum in the corner
    apex: usize,
    /// whether the reference has its lowest speed inside the corner
    has_minimum: bool,
    max_curvature: f32,
    segment_start: usize,
}

/// Finds the corners on the first lap (the reference) and measures every lap
/// in them.
///
/// The reference needs `t`, `speed` and `g_lat`; without `g_lat` (some
/// imported files) [`AnalysisError::MissingChannel`] is returned. Laps
/// without pedals get `None` for the pedal statistics.
pub fn corners(laps: &[(&Lap, &LapTelemetry)]) -> Result<CornerReport, AnalysisError> {
    let Some((_, reference)) = laps.first() else {
        return Ok(CornerReport {
            corners: Vec::new(),
            laps: Vec::new(),
        });
    };
    if reference.channel(names::G_LAT).is_none() {
        return Err(AnalysisError::MissingChannel(names::G_LAT));
    }
    if reference.channel(names::SPEED).is_none() {
        return Err(AnalysisError::MissingChannel(names::SPEED));
    }
    let len = common_grid_len(laps.iter().map(|(_, t)| *t), STEP_M);
    let traces = laps
        .iter()
        .map(|(_, t)| resample(t, STEP_M, Some(len), Some(CHANNELS)).map(|r| Trace::new(&r, len)))
        .collect::<Result<Vec<_>, _>>()?;

    let reference = &traces[0];
    let mut regions = find_regions(reference);

    // A segment starts before the earliest braking or lift of all laps, so
    // a lap that brakes early loses that time in the right corner.
    let margin = (APPROACH_MARGIN_M / STEP_M) as usize;
    let measure_all = |regions: &[Region]| -> Vec<Vec<CornerLapStats>> {
        traces
            .iter()
            .map(|t| (0..regions.len()).map(|i| measure(t, regions, i)).collect())
            .collect()
    };
    let first_stats = measure_all(&regions);
    for i in 0..regions.len() {
        let earliest = first_stats
            .iter()
            .flat_map(|s| [s[i].brake_m, s[i].lift_m])
            .flatten()
            .map(|m| (m / STEP_M) as usize)
            .min();
        let Some(e) = earliest else { continue };
        let wanted = regions[i].segment_start.min(e.saturating_sub(margin));
        let start = match i.checked_sub(1).map(|p| &mut regions[p]) {
            None => wanted,
            // a bend the car brakes out of for this corner: its apex moves
            // before the earliest braking too
            Some(p) if !p.has_minimum => {
                let start = wanted.max(p.start + 1).max(p.segment_start + 1);
                p.apex = p.apex.min(start - 1);
                start
            }
            Some(p) => wanted.max(p.apex + 1),
        };
        regions[i].segment_start = start;
    }
    let mut stats = measure_all(&regions);

    for (trace, lap_stats) in traces.iter().zip(&mut stats) {
        for (i, s) in lap_stats.iter_mut().enumerate() {
            s.time_s = segment_time(trace, &regions, i);
        }
    }
    let reference_times: Vec<Option<f32>> = stats[0].iter().map(|s| s.time_s).collect();
    for (trace, lap_stats) in traces.iter().zip(&mut stats) {
        for (i, s) in lap_stats.iter_mut().enumerate() {
            s.time_lost_s = sub(s.time_s, reference_times[i]);
            let entry = sub(
                entry_time(trace, &regions[i]),
                entry_time(reference, &regions[i]),
            );
            s.entry_lost_s = entry;
            s.exit_lost_s = sub(s.time_lost_s, entry);
        }
    }

    let corners = regions
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let next = regions.get(i + 1).unwrap_or(&regions[0]);
            Corner {
                number: i as u32 + 1,
                direction: r.direction,
                start_m: r.start as f32 * STEP_M,
                apex_m: r.apex as f32 * STEP_M,
                end_m: (r.end - 1) as f32 * STEP_M,
                segment_start_m: r.segment_start as f32 * STEP_M,
                segment_end_m: next.segment_start as f32 * STEP_M,
                min_radius_m: 1.0 / r.max_curvature,
            }
        })
        .collect();
    let laps = laps
        .iter()
        .zip(stats)
        .map(|((lap, _), corners)| LapCorners {
            lap_id: lap.id,
            corners,
        })
        .collect();
    Ok(CornerReport { corners, laps })
}

/// The channels of one lap on the grid; missing ones are all `NaN`.
struct Trace {
    t: Vec<f32>,
    speed: Vec<f32>,
    throttle: Vec<f32>,
    brake: Vec<f32>,
    gear: Vec<f32>,
    g_lat: Vec<f32>,
}

impl Trace {
    fn new(r: &Resampled, len: usize) -> Self {
        let ch = |name| {
            r.channel(name)
                .map_or_else(|| vec![f32::NAN; len], <[f32]>::to_vec)
        };
        Self {
            t: ch(names::TIME),
            speed: ch(names::SPEED),
            throttle: ch(names::THROTTLE),
            brake: ch(names::BRAKE),
            gear: ch(names::GEAR),
            g_lat: ch(names::G_LAT),
        }
    }

    fn len(&self) -> usize {
        self.t.len()
    }

    fn time(&self, i: usize) -> Option<f32> {
        self.t.get(i).copied().filter(|t| t.is_finite())
    }

    /// Time at the start/finish line: 0 for a lap whose data starts there
    /// (`t` counts from the lap start), `None` for one that joined the track
    /// later (out-lap).
    fn start_time(&self) -> Option<f32> {
        let first = self.t.iter().position(|t| t.is_finite())?;
        (first as f32 * STEP_M <= LAP_START_M).then_some(0.0)
    }

    /// Time of the last point with data (the end of the lap).
    fn end_time(&self) -> Option<f32> {
        self.t.iter().rev().copied().find(|t| t.is_finite())
    }
}

/// Signed curvature (1/m, positive = right-hand), smoothed; `NaN` where the
/// lap has no data or is too slow.
fn curvature(trace: &Trace) -> Vec<f32> {
    let raw: Vec<f32> = trace
        .g_lat
        .iter()
        .zip(&trace.speed)
        .map(|(&g, &v)| {
            if v.is_finite() && v >= MIN_CURVATURE_SPEED {
                g / (v * v)
            } else {
                f32::NAN
            }
        })
        .collect();
    smooth(&raw, SMOOTH_HALF)
}

/// Moving average over `±half` points, ignoring `NaN`; `NaN` where the
/// window has no values.
fn smooth(v: &[f32], half: usize) -> Vec<f32> {
    let mut sum = vec![0.0f64; v.len() + 1];
    let mut count = vec![0u32; v.len() + 1];
    for (i, &x) in v.iter().enumerate() {
        let ok = x.is_finite();
        sum[i + 1] = sum[i] + if ok { f64::from(x) } else { 0.0 };
        count[i + 1] = count[i] + u32::from(ok);
    }
    (0..v.len())
        .map(|i| {
            if !v[i].is_finite() {
                return f32::NAN;
            }
            let a = i.saturating_sub(half);
            let b = (i + half + 1).min(v.len());
            let n = count[b] - count[a];
            if n == 0 {
                f32::NAN
            } else {
                ((sum[b] - sum[a]) / f64::from(n)) as f32
            }
        })
        .collect()
}

/// Corners of the reference lap, with their segments.
fn find_regions(reference: &Trace) -> Vec<Region> {
    let k = curvature(reference);
    let edge = 1.0 / CORNER_EDGE_RADIUS_M;
    let core = 1.0 / CORNER_RADIUS_M;

    // runs of the same direction above the edge curvature
    let mut runs: Vec<(TurnDirection, usize, usize, f32)> = Vec::new();
    let mut current: Option<(TurnDirection, usize, f32)> = None;
    for (i, &c) in k.iter().chain(std::iter::once(&f32::NAN)).enumerate() {
        let dir = if c.is_finite() && c.abs() > edge {
            Some(if c > 0.0 {
                TurnDirection::Right
            } else {
                TurnDirection::Left
            })
        } else {
            None
        };
        match (current, dir) {
            (Some((d, s, m)), Some(nd)) if d == nd => current = Some((d, s, m.max(c.abs()))),
            (cur, nd) => {
                if let Some((d, s, m)) = cur {
                    runs.push((d, s, i, m));
                }
                current = nd.map(|d| (d, i, c.abs()));
            }
        }
    }

    // merge double apexes, then keep real corners
    let gap = (MERGE_GAP_M / STEP_M) as usize;
    let mut merged: Vec<(TurnDirection, usize, usize, f32)> = Vec::new();
    for run in runs {
        match merged.last_mut() {
            Some(last) if last.0 == run.0 && run.1 - last.2 < gap => {
                last.2 = run.2;
                last.3 = last.3.max(run.3);
            }
            _ => merged.push(run),
        }
    }
    let min_len = (MIN_CORNER_LEN_M / STEP_M) as usize;
    let kept: Vec<_> = merged
        .into_iter()
        .filter(|r| r.3 >= core && r.2 - r.1 >= min_len)
        .collect();

    let margin = (APPROACH_MARGIN_M / STEP_M) as usize;
    let around = (LOCAL_MIN_M / STEP_M) as usize;
    // apex candidates: the slowest point if it is a real minimum, else the
    // tightest point
    let first_pass: Vec<(usize, bool)> = kept
        .iter()
        .map(|&(_, start, end, _)| {
            let tightest = argmax_abs(&k, start..end).unwrap_or(start);
            match argmin(&reference.speed, start..end) {
                Some(i)
                    if min(&reference.speed, i.saturating_sub(around)..i + around + 1)
                        .is_some_and(|m| m >= reference.speed[i]) =>
                {
                    (i, true)
                }
                _ => (tightest, false),
            }
        })
        .collect();

    let braking_bend = (BRAKING_BEND_M / STEP_M) as usize;
    let mut regions: Vec<Region> = Vec::with_capacity(kept.len());
    // braking can't start before the previous real minimum
    let mut floor = 0;
    for (n, &(direction, start, end, max_curvature)) in kept.iter().enumerate() {
        let (mut apex, has_minimum) = first_pass[n];
        if !has_minimum
            && let Some(&(next_apex, _)) = first_pass.get(n + 1)
            && let Some(b) = brake_start(reference, floor, next_apex)
            && b <= apex
        {
            // the car is braking for the next corner in this one
            if b < start + braking_bend {
                continue; // a bend in the braking zone
            }
            // a long bend into a slow corner: its apex is before the braking
            apex = b.saturating_sub(margin).max(start);
        }
        let after_previous = regions.last().map_or(0, |p| p.apex + 1);
        let apex = apex.max(after_previous);
        let brake = if has_minimum {
            brake_start(reference, floor, apex)
        } else {
            None
        };
        let lift = lift_point(reference, floor.max(after_previous), brake.unwrap_or(apex));
        let earliest = [Some(start), brake, lift].into_iter().flatten().min();
        let segment_start = earliest
            .unwrap_or(start)
            .saturating_sub(margin)
            .max(after_previous);
        if has_minimum {
            floor = apex + 1;
        }
        regions.push(Region {
            direction,
            start,
            end,
            apex,
            has_minimum,
            max_curvature,
            segment_start,
        });
    }
    regions
}

/// Start of the braking that leads into `apex`: the last braking before it,
/// followed back to where it begins (short gaps don't end it), not before
/// `from`.
fn brake_start(trace: &Trace, from: usize, apex: usize) -> Option<usize> {
    let gap = (BRAKE_GAP_M / STEP_M) as usize;
    let mut j = last(&trace.brake, from..apex + 1, |b| b >= BRAKE_ON)?;
    let mut since = 0;
    let mut start = j;
    while j > from {
        j -= 1;
        if trace.brake[j] >= BRAKE_ON {
            start = j;
            since = 0;
        } else {
            since += 1;
            if since > gap {
                break;
            }
        }
    }
    Some(start)
}

/// Where the braking that starts at `start` ends (short gaps don't end it),
/// before `until`.
fn brake_end(trace: &Trace, start: usize, until: usize) -> usize {
    let gap = (BRAKE_GAP_M / STEP_M) as usize;
    let mut end = start + 1;
    let mut since = 0;
    for j in start + 1..until.min(trace.len()) {
        if trace.brake[j] >= BRAKE_ON {
            end = j + 1;
            since = 0;
        } else {
            since += 1;
            if since > gap {
                break;
            }
        }
    }
    end
}

/// Time spent in the segment of corner `i`.
fn segment_time(trace: &Trace, regions: &[Region], i: usize) -> Option<f32> {
    let r = &regions[i];
    match regions.get(i + 1) {
        Some(n) => sub(trace.time(n.segment_start), trace.time(r.segment_start)),
        // the last segment wraps around the start/finish line
        None => {
            let to_end = sub(trace.end_time(), trace.time(r.segment_start));
            let from_start = sub(trace.time(regions[0].segment_start), trace.start_time());
            to_end.zip(from_start).map(|(a, b)| a + b)
        }
    }
}

/// Measures corner `i` on one lap (without the times).
fn measure(trace: &Trace, regions: &[Region], i: usize) -> CornerLapStats {
    let r = &regions[i];
    let next_start = next_segment_start(trace, regions, i);
    let corner = corner_range(trace, regions, i);
    let m = |i: usize| i as f32 * STEP_M;

    let Some(min_idx) = lap_minimum(trace, regions, i) else {
        return CornerLapStats::default();
    };
    // this lap's braking can't start before its previous real minimum
    let floor = (0..i)
        .rev()
        .find(|&j| regions[j].has_minimum)
        .and_then(|j| lap_minimum(trace, regions, j))
        .map_or(0, |j| j + 1);
    // in a corner without a minimum of its own, braking is for the next one
    let brake = if r.has_minimum {
        brake_start(trace, floor, min_idx)
    } else {
        None
    };
    let brake_release = brake.map(|b| brake_end(trace, b, corner.end));
    let brake_max = brake
        .zip(brake_release)
        .and_then(|(b, e)| max(&trace.brake, b..e));
    let has_pedals = trace.throttle.iter().any(|t| t.is_finite());
    let flat_out = has_pedals
        && brake.is_none()
        && min(
            &trace.throttle,
            floor.max(r.start.saturating_sub(50))..corner.end,
        )
        .is_some_and(|t| t >= LIFT);
    let lift = (!flat_out)
        .then(|| {
            let after_previous = i.checked_sub(1).map_or(0, |p| regions[p].apex + 1);
            lift_point(trace, floor.max(after_previous), brake.unwrap_or(min_idx))
        })
        .flatten();

    let (throttle, full_throttle) = if flat_out || !has_pedals {
        (None, None)
    } else {
        let full = first(&trace.throttle, min_idx..next_start, |t| t >= FULL);
        // walk back from full throttle (or the segment end) to where the
        // driver got on the throttle: skips blips while braking and partial
        // throttle mid-corner. None if they never got off it (only a partial
        // lift: see lift and full throttle).
        let from = full.unwrap_or(next_start.saturating_sub(1));
        let floor = brake.unwrap_or(floor);
        let mut on = None;
        if trace.throttle.get(from).is_some_and(|&t| t >= THROTTLE_ON) {
            let mut j = from;
            while j > floor {
                let t = trace.throttle[j - 1];
                if t < THROTTLE_ON {
                    on = Some(j);
                    break;
                }
                if t >= FULL && j < from {
                    break;
                }
                j -= 1;
            }
        }
        (on, full)
    };

    CornerLapStats {
        time_s: None,
        time_lost_s: None,
        entry_lost_s: None,
        exit_lost_s: None,
        top_speed_mps: max(&trace.speed, floor..min_idx + 1),
        lift_m: lift.map(m),
        brake_m: brake.map(m),
        brake_max,
        brake_release_m: brake_release.map(m),
        min_speed_mps: trace.speed.get(min_idx).copied(),
        min_speed_m: Some(m(min_idx)),
        min_speed_gear: trace
            .gear
            .get(min_idx)
            .copied()
            .filter(|g| g.is_finite())
            .map(|g| g as i32),
        throttle_m: throttle.map(m),
        full_throttle_m: full_throttle.map(m),
        exit_speed_mps: trace
            .speed
            .get(corner.end - 1)
            .copied()
            .filter(|v| v.is_finite()),
        flat_out,
    }
}

/// Start of the segment after corner `i` (the end of the lap for the last).
fn next_segment_start(trace: &Trace, regions: &[Region], i: usize) -> usize {
    regions.get(i + 1).map_or(trace.len(), |n| n.segment_start)
}

/// Corner `i` as far as it isn't the approach to the next one.
fn corner_range(trace: &Trace, regions: &[Region], i: usize) -> Range<usize> {
    let r = &regions[i];
    r.start
        ..r.end
            .min(next_segment_start(trace, regions, i))
            .max(r.apex + 1)
}

/// Where the lap is slowest in corner `i`; at the apex for a corner without
/// a minimum of its own.
fn lap_minimum(trace: &Trace, regions: &[Region], i: usize) -> Option<usize> {
    let r = &regions[i];
    if r.has_minimum {
        argmin(&trace.speed, corner_range(trace, regions, i))
    } else {
        trace.speed[r.apex].is_finite().then_some(r.apex)
    }
}

/// Time from the segment start to the reference's apex.
fn entry_time(trace: &Trace, r: &Region) -> Option<f32> {
    sub(trace.time(r.apex), trace.time(r.segment_start))
}

/// Where the throttle drops below [`LIFT`] for the last time before `until`,
/// searching from `from`. `None` if it isn't at full throttle in between.
fn lift_point(trace: &Trace, from: usize, until: usize) -> Option<usize> {
    last(&trace.throttle, from..until, |t| t >= LIFT)
        .map(|j| j + 1)
        .filter(|&j| j < until)
}

fn sub(a: Option<f32>, b: Option<f32>) -> Option<f32> {
    a.zip(b).map(|(a, b)| a - b)
}

fn window(v: &[f32], range: Range<usize>) -> impl Iterator<Item = (usize, f32)> + '_ {
    // empty for reversed ranges (erratic laps: spins, cutting the track)
    let end = range.end.min(v.len());
    let range = range.start.min(end)..end;
    v[range.clone()]
        .iter()
        .copied()
        .enumerate()
        .map(move |(i, x)| (range.start + i, x))
        .filter(|(_, x)| x.is_finite())
}

fn first(v: &[f32], range: Range<usize>, pred: impl Fn(f32) -> bool) -> Option<usize> {
    window(v, range).find(|&(_, x)| pred(x)).map(|(i, _)| i)
}

fn last(v: &[f32], range: Range<usize>, pred: impl Fn(f32) -> bool) -> Option<usize> {
    window(v, range)
        .filter(|&(_, x)| pred(x))
        .last()
        .map(|(i, _)| i)
}

fn argmin(v: &[f32], range: Range<usize>) -> Option<usize> {
    window(v, range)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

fn argmax_abs(v: &[f32], range: Range<usize>) -> Option<usize> {
    window(v, range)
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .map(|(i, _)| i)
}

fn min(v: &[f32], range: Range<usize>) -> Option<f32> {
    window(v, range).map(|(_, x)| x).min_by(f32::total_cmp)
}

fn max(v: &[f32], range: Range<usize>) -> Option<f32> {
    window(v, range).map(|(_, x)| x).max_by(f32::total_cmp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sy_model::session::LapFlags;
    use sy_model::{BlobHash, Channel, ChannelData, LapId, SessionId, Unit};

    /// A bend of a synthetic track.
    #[derive(Clone, Copy)]
    struct Turn {
        start: f32,
        end: f32,
        /// radius, positive = right-hand
        radius: f32,
        /// speed through the bend, m/s
        speed: f32,
        /// deceleration when braking for it, m/s²
        decel: f32,
    }

    const TRACK_M: f32 = 2000.0;
    const V_MAX: f32 = 50.0;
    const ACCEL: f32 = 5.0;

    fn turn(start: f32, end: f32, radius: f32, speed: f32) -> Turn {
        Turn {
            start,
            end,
            radius,
            speed,
            decel: 15.0,
        }
    }

    /// Speed at `d`: the slowest of top speed, braking for, driving through
    /// and accelerating out of every turn.
    fn speed_at(turns: &[Turn], d: f32) -> (f32, bool) {
        let mut v = V_MAX;
        let mut braking = false;
        for t in turns {
            let limit = if d < t.start {
                (t.speed * t.speed + 2.0 * t.decel * (t.start - d)).sqrt()
            } else if d <= t.end {
                t.speed
            } else {
                (t.speed * t.speed + 2.0 * ACCEL * (d - t.end)).sqrt()
            };
            if limit < v {
                v = limit;
                braking = d < t.start;
            }
        }
        (v, braking)
    }

    /// Curvature at `d`, with 15 m transitions into and out of each turn.
    fn curvature_at(turns: &[Turn], d: f32) -> f32 {
        turns
            .iter()
            .map(|t| {
                let ramp = 15.0;
                let w = if d < t.start - ramp || d > t.end + ramp {
                    0.0
                } else if d < t.start {
                    (d - (t.start - ramp)) / ramp
                } else if d > t.end {
                    ((t.end + ramp) - d) / ramp
                } else {
                    1.0
                };
                w / t.radius
            })
            .sum()
    }

    /// A lap of the synthetic track, sampled every 0.5 m.
    fn lap(turns: &[Turn]) -> LapTelemetry {
        let n = (TRACK_M / 0.5) as usize + 1;
        let (mut t, mut d, mut v, mut thr, mut brk, mut lat) =
            (vec![], vec![], vec![], vec![], vec![], vec![]);
        let mut time = 0.0f32;
        for i in 0..n {
            let dist = i as f32 * 0.5;
            let (speed, braking) = speed_at(turns, dist);
            if i > 0 {
                time += 0.5 / ((speed + v[i - 1]) / 2.0);
            }
            let in_turn = turns.iter().any(|x| (x.start..=x.end).contains(&dist));
            t.push(time);
            d.push(dist);
            v.push(speed);
            brk.push(if braking { 0.8 } else { 0.0 });
            thr.push(if braking {
                0.0
            } else if in_turn && speed < V_MAX {
                0.3
            } else {
                1.0
            });
            lat.push(curvature_at(turns, dist) * speed * speed);
        }
        let f = |name, unit, v| Channel::new(name, unit, ChannelData::F32(v));
        LapTelemetry::new(60.0)
            .with_channel(f(names::TIME, Unit::Second, t))
            .unwrap()
            .with_channel(f(names::DIST, Unit::Meter, d))
            .unwrap()
            .with_channel(f(names::SPEED, Unit::MeterPerSecond, v))
            .unwrap()
            .with_channel(f(names::THROTTLE, Unit::Ratio, thr))
            .unwrap()
            .with_channel(f(names::BRAKE, Unit::Ratio, brk))
            .unwrap()
            .with_channel(f(names::G_LAT, Unit::MeterPerSecondSquared, lat))
            .unwrap()
    }

    fn meta() -> Lap {
        Lap {
            id: LapId::new(),
            session_id: SessionId::new(),
            number: 1,
            lap_time_ms: None,
            sector_times_ms: vec![],
            valid: true,
            invalid_reason: None,
            flags: LapFlags::default(),
            blob_hash: BlobHash::of(b""),
            sample_rate_hz: 60.0,
            sample_count: 0,
            start_time_s: None,
        }
    }

    fn track() -> Vec<Turn> {
        vec![
            turn(400.0, 450.0, 30.0, 15.0),     // slow right-hander
            turn(1100.0, 1200.0, -100.0, 30.0), // medium left-hander
            turn(1600.0, 1650.0, 200.0, V_MAX), // flat-out kink
        ]
    }

    fn end_time(l: &LapTelemetry) -> f32 {
        match &l.channel(names::TIME).unwrap().data {
            ChannelData::F32(t) => *t.last().unwrap(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn finds_and_measures_corners() {
        let l = lap(&track());
        let r = corners(&[(&meta(), &l)]).unwrap();
        let c = &r.corners;
        assert_eq!(c.len(), 3, "{c:#?}");
        assert_eq!(c[0].direction, TurnDirection::Right);
        assert_eq!(c[1].direction, TurnDirection::Left);
        assert!((c[0].min_radius_m - 30.0).abs() < 3.0, "{c:#?}");
        // turn-in: the transition into the bend, blurred by the smoothing
        assert!((375.0..=400.0).contains(&c[0].start_m), "{c:#?}");

        let s = &r.laps[0].corners;
        // braking from 50 to 15 m/s at 15 m/s² takes 76 m
        let brake = s[0].brake_m.unwrap();
        assert!((320.0..=330.0).contains(&brake), "{s:#?}");
        assert!((s[0].min_speed_mps.unwrap() - 15.0).abs() < 0.1);
        let apex = s[0].min_speed_m.unwrap();
        assert!((400.0..=450.0).contains(&apex), "{s:#?}");
        assert_eq!(s[0].brake_release_m, Some(400.0));
        assert!(s[0].full_throttle_m.unwrap() > 450.0);
        assert!(!s[0].flat_out);
        assert_eq!(s[0].time_lost_s, Some(0.0));
        assert!(c[0].segment_start_m < brake);

        // the kink: no braking, no lift
        assert!(s[2].flat_out, "{s:#?}");
        assert_eq!(s[2].brake_m, None);
        assert_eq!(s[2].lift_m, None);

        // the segments cover the lap
        let total: f32 = s.iter().map(|x| x.time_s.unwrap()).sum();
        assert!((total - end_time(&l)).abs() < 0.01, "{total}");
    }

    #[test]
    fn time_lost_is_where_the_lap_is_slower() {
        let reference = lap(&track());
        let mut slow = track();
        slow[1].decel = 8.0; // brakes much earlier for the left-hander
        let slow = lap(&slow);
        let r = corners(&[(&meta(), &reference), (&meta(), &slow)]).unwrap();
        let (fast, slow_stats) = (&r.laps[0].corners, &r.laps[1].corners);

        let lost: Vec<f32> = slow_stats.iter().map(|c| c.time_lost_s.unwrap()).collect();
        assert!(lost[1] > 0.2, "{lost:?}");
        assert!(lost[0].abs() < 0.01 && lost[2].abs() < 0.01, "{lost:?}");
        let gap = end_time(&slow) - end_time(&reference);
        assert!((lost.iter().sum::<f32>() - gap).abs() < 0.01);

        // all of it before the apex
        let c = &slow_stats[1];
        assert!((c.entry_lost_s.unwrap() - lost[1]).abs() < 0.01, "{c:#?}");
        assert!(c.brake_m.unwrap() < fast[1].brake_m.unwrap() - 30.0);
        assert!((c.min_speed_mps.unwrap() - fast[1].min_speed_mps.unwrap()).abs() < 0.1);
    }

    #[test]
    fn double_apex_is_one_corner() {
        let turns = vec![
            turn(400.0, 450.0, 40.0, 18.0),
            turn(470.0, 520.0, 40.0, 18.0),
        ];
        let r = corners(&[(&meta(), &lap(&turns))]).unwrap();
        assert_eq!(r.corners.len(), 1, "{:#?}", r.corners);
    }

    #[test]
    fn chicane_is_two_corners() {
        let turns = vec![
            turn(400.0, 430.0, 40.0, 20.0),
            turn(450.0, 480.0, -40.0, 24.0), // no braking between
        ];
        let r = corners(&[(&meta(), &lap(&turns))]).unwrap();
        let dirs: Vec<_> = r.corners.iter().map(|c| c.direction).collect();
        assert_eq!(dirs, [TurnDirection::Right, TurnDirection::Left]);
        // one braking, for the first part
        let s = &r.laps[0].corners;
        assert!(s[0].brake_m.is_some());
        assert_eq!(s[1].brake_m, None);
    }

    #[test]
    fn needs_lateral_acceleration() {
        let l = LapTelemetry::new(60.0)
            .with_channel(Channel::new(
                names::SPEED,
                Unit::MeterPerSecond,
                ChannelData::F32(vec![1.0]),
            ))
            .unwrap();
        assert_eq!(
            corners(&[(&meta(), &l)]),
            Err(AnalysisError::MissingChannel(names::G_LAT))
        );
        assert!(corners(&[]).unwrap().corners.is_empty());
    }
}
