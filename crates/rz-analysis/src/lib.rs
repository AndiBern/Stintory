// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Lap analysis (docs/PLAN.md §6).
//!
//! - [`resample`]: puts a lap onto a fixed distance grid, so laps can be
//!   compared point by point.
//! - [`delta`]: time difference between two resampled laps.
//! - [`compare`]: builds the [`CompareData`] the frontend draws.
//!
//! Used by both the desktop backend and rz-server, so the analysis exists
//! exactly once.

use std::collections::BTreeMap;

use rz_model::api::{CompareData, LapTrace};
use rz_model::channel::names;
use rz_model::{ChannelData, Lap, LapTelemetry, Unit};

/// Default grid resolution in metres.
pub const DEFAULT_STEP_M: f32 = 1.0;
/// Smallest allowed grid resolution (protects against huge responses).
pub const MIN_STEP_M: f32 = 0.25;

/// Channels returned by [`compare`] (if the laps have them).
pub const COMPARE_CHANNELS: &[&str] = &[
    names::TIME,
    names::SPEED,
    names::THROTTLE,
    names::BRAKE,
    names::CLUTCH,
    names::STEER,
    names::GEAR,
    names::RPM,
    names::G_LAT,
    names::G_LONG,
    names::POS_X,
    names::POS_Y,
    names::ABS_ACTIVE,
    names::FUEL,
];

/// Errors of the analysis.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AnalysisError {
    /// The lap has no `dist` or `t` channel.
    #[error("lap has no {0} channel")]
    MissingChannel(&'static str),
    /// Invalid grid resolution.
    #[error("invalid step {0} m")]
    InvalidStep(f32),
}

/// A lap on a distance grid. All vectors have the same length; grid point
/// `i` is at `i * step_m` metres. `NaN` where the lap has no data (e.g. the
/// part of a lap before an out-lap joined the track).
#[derive(Debug, Clone, PartialEq)]
pub struct Resampled {
    /// Grid resolution
    pub step_m: f32,
    /// Resampled channels: name, unit, values
    pub channels: Vec<(String, Unit, Vec<f32>)>,
}

impl Resampled {
    /// Number of grid points.
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, |c| c.2.len())
    }

    /// True if there are no grid points.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Values of a channel.
    pub fn channel(&self, name: &str) -> Option<&[f32]> {
        self.channels
            .iter()
            .find(|c| c.0 == name)
            .map(|c| c.2.as_slice())
    }
}

fn as_f32(data: &ChannelData) -> Vec<f32> {
    match data {
        ChannelData::F32(v) => v.clone(),
        ChannelData::F64(v) => v.iter().map(|&x| x as f32).collect(),
        ChannelData::I32(v) => v.iter().map(|&x| x as f32).collect(),
        ChannelData::Bool(v) => v.iter().map(|&x| f32::from(u8::from(x))).collect(),
    }
}

/// Resamples a lap onto a distance grid from 0 to `grid_len` points.
///
/// Only samples where the distance strictly increases are used (standing
/// still or rolling backwards would make the distance ambiguous). Continuous
/// channels are interpolated linearly; integer and boolean channels (gear,
/// flags) keep the value of the previous sample. Channels not in
/// `wanted` are skipped (`None` = all).
pub fn resample(
    lap: &LapTelemetry,
    step_m: f32,
    grid_len: Option<usize>,
    wanted: Option<&[&str]>,
) -> Result<Resampled, AnalysisError> {
    if !(step_m.is_finite() && step_m >= MIN_STEP_M) {
        return Err(AnalysisError::InvalidStep(step_m));
    }
    let dist = lap
        .channel(names::DIST)
        .ok_or(AnalysisError::MissingChannel(names::DIST))?;
    lap.channel(names::TIME)
        .ok_or(AnalysisError::MissingChannel(names::TIME))?;
    let dist = as_f32(&dist.data);

    // indices of samples with strictly increasing distance
    let mut kept: Vec<usize> = Vec::with_capacity(dist.len());
    for (i, &d) in dist.iter().enumerate() {
        if d.is_finite() && kept.last().is_none_or(|&k| d > dist[k]) {
            kept.push(i);
        }
    }
    let max_dist = kept.last().map_or(0.0, |&k| dist[k]);
    let len = grid_len.unwrap_or_else(|| (max_dist / step_m).floor() as usize + 1);

    // For every grid point: the kept-sample segment (a, b) and the weight.
    let mut segments: Vec<Option<(usize, usize, f32)>> = Vec::with_capacity(len);
    let mut j = 0usize;
    for g in 0..len {
        let d = g as f32 * step_m;
        while j + 1 < kept.len() && dist[kept[j + 1]] < d {
            j += 1;
        }
        let seg = match (kept.get(j), kept.get(j + 1)) {
            (Some(&a), Some(&b)) if dist[a] <= d && d <= dist[b] => {
                Some((a, b, (d - dist[a]) / (dist[b] - dist[a])))
            }
            (Some(&a), _) if dist[a] == d => Some((a, a, 0.0)),
            _ => None,
        };
        segments.push(seg);
    }

    let mut channels = Vec::new();
    for ch in lap.channels() {
        if ch.name == names::DIST || wanted.is_some_and(|w| !w.contains(&ch.name.as_str())) {
            continue;
        }
        let stepwise = matches!(ch.data, ChannelData::I32(_) | ChannelData::Bool(_));
        let v = as_f32(&ch.data);
        let values = segments
            .iter()
            .map(|seg| match *seg {
                Some((a, _, _)) if stepwise => v[a],
                Some((a, b, w)) => v[a] + (v[b] - v[a]) * w,
                None => f32::NAN,
            })
            .collect();
        channels.push((ch.name.clone(), ch.unit, values));
    }
    Ok(Resampled { step_m, channels })
}

/// Time difference `lap − reference` at each grid point, in seconds
/// (positive = the lap is slower). `NaN` where either lap has no data.
pub fn delta(lap_time: &[f32], reference_time: &[f32]) -> Vec<f32> {
    lap_time
        .iter()
        .zip(reference_time)
        .map(|(&a, &b)| a - b)
        .collect()
}

/// Resamples the given laps onto a common grid and computes each lap's delta
/// to the first lap (the reference).
pub fn compare(laps: &[(&Lap, &LapTelemetry)], step_m: f32) -> Result<CompareData, AnalysisError> {
    // common grid: long enough for the longest lap
    let mut grid_len = 0;
    for (_, t) in laps {
        if let Some(dist) = t.channel(names::DIST) {
            let max = as_f32(&dist.data)
                .into_iter()
                .filter(|d| d.is_finite())
                .fold(0.0f32, f32::max);
            grid_len = grid_len.max((max / step_m).floor() as usize + 1);
        }
    }

    let mut units = BTreeMap::new();
    let mut traces = Vec::new();
    let mut reference_time: Option<Vec<f32>> = None;
    for (lap, telemetry) in laps {
        let r = resample(telemetry, step_m, Some(grid_len), Some(COMPARE_CHANNELS))?;
        let time = r
            .channel(names::TIME)
            .map(<[f32]>::to_vec)
            .unwrap_or_else(|| vec![f32::NAN; grid_len]);
        let reference = reference_time.get_or_insert_with(|| time.clone());
        let delta_s = delta(&time, reference);
        let mut channels = BTreeMap::new();
        for (name, unit, values) in r.channels {
            units.insert(name.clone(), unit.as_str().to_owned());
            channels.insert(name, values);
        }
        traces.push(LapTrace {
            lap_id: lap.id,
            session_id: lap.session_id,
            number: lap.number,
            lap_time_ms: lap.lap_time_ms,
            valid: lap.valid,
            channels,
            delta_s,
        });
    }
    Ok(CompareData {
        step_m,
        dist_m: (0..grid_len).map(|g| g as f32 * step_m).collect(),
        units,
        laps: traces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rz_model::session::LapFlags;
    use rz_model::{BlobHash, Channel, LapId, SessionId};

    /// Lap of `len` metres driven at constant `speed`, sampled at 10 Hz.
    fn lap(len: f32, speed: f32) -> LapTelemetry {
        let n = (len / speed * 10.0) as usize + 1;
        let t: Vec<f32> = (0..n).map(|i| i as f32 / 10.0).collect();
        let d: Vec<f32> = t.iter().map(|t| t * speed).collect();
        let gear: Vec<i32> = d
            .iter()
            .map(|d| if *d < len / 2.0 { 3 } else { 4 })
            .collect();
        LapTelemetry::new(10.0)
            .with_channel(Channel::new(names::TIME, Unit::Second, ChannelData::F32(t)))
            .unwrap()
            .with_channel(Channel::new(names::DIST, Unit::Meter, ChannelData::F32(d)))
            .unwrap()
            .with_channel(Channel::new(
                names::SPEED,
                Unit::MeterPerSecond,
                ChannelData::F32(vec![speed; n]),
            ))
            .unwrap()
            .with_channel(Channel::new(
                names::GEAR,
                Unit::None,
                ChannelData::I32(gear),
            ))
            .unwrap()
    }

    fn meta(n: u32) -> Lap {
        Lap {
            id: LapId::new(),
            session_id: SessionId::new(),
            number: n,
            lap_time_ms: Some(1000),
            sector_times_ms: vec![],
            valid: true,
            invalid_reason: None,
            flags: LapFlags::default(),
            blob_hash: BlobHash::of(b""),
            sample_rate_hz: 10.0,
            sample_count: 0,
            start_time_s: None,
        }
    }

    #[test]
    fn resamples_onto_distance_grid() {
        let r = resample(&lap(1000.0, 50.0), 1.0, None, None).unwrap();
        assert_eq!(r.len(), 1001);
        let t = r.channel(names::TIME).unwrap();
        assert!((t[500] - 10.0).abs() < 1e-4, "500 m at 50 m/s = 10 s");
        assert!((t[1000] - 20.0).abs() < 1e-4);
        let gear = r.channel(names::GEAR).unwrap();
        assert_eq!(gear[100], 3.0);
        assert_eq!(gear[900], 4.0);
        assert!(r.channel(names::DIST).is_none(), "the grid replaces dist");
    }

    #[test]
    fn ignores_standing_still_and_rolling_back() {
        let t = vec![0.0, 1.0, 2.0, 3.0, 4.0];
        let d = vec![0.0, 10.0, 10.0, 8.0, 20.0]; // stop, roll back, go
        let lap = LapTelemetry::new(1.0)
            .with_channel(Channel::new(names::TIME, Unit::Second, ChannelData::F32(t)))
            .unwrap()
            .with_channel(Channel::new(names::DIST, Unit::Meter, ChannelData::F32(d)))
            .unwrap();
        let r = resample(&lap, 1.0, None, None).unwrap();
        let time = r.channel(names::TIME).unwrap();
        assert_eq!(time.len(), 21);
        assert_eq!(time[10], 1.0);
        assert!(
            (time[15] - 2.5).abs() < 1e-6,
            "interpolated 10 m (1 s) → 20 m (4 s)"
        );
    }

    #[test]
    fn grid_outside_the_lap_is_nan() {
        // out-lap that joins the track at 300 m and ends at 1000 m
        let t: Vec<f32> = (0..=70).map(|i| i as f32).collect();
        let d: Vec<f32> = t.iter().map(|t| 300.0 + t * 10.0).collect();
        let lap = LapTelemetry::new(1.0)
            .with_channel(Channel::new(names::TIME, Unit::Second, ChannelData::F32(t)))
            .unwrap()
            .with_channel(Channel::new(names::DIST, Unit::Meter, ChannelData::F32(d)))
            .unwrap();
        let r = resample(&lap, 1.0, Some(1200), None).unwrap();
        let time = r.channel(names::TIME).unwrap();
        assert!(time[299].is_nan());
        assert_eq!(time[300], 0.0);
        assert_eq!(time[1000], 70.0);
        assert!(time[1100].is_nan(), "after the lap's end");
    }

    #[test]
    fn delta_shows_where_time_is_lost() {
        let fast = lap(1000.0, 50.0);
        let slow = lap(1000.0, 40.0);
        let (a, b) = (meta(1), meta(2));
        let c = compare(&[(&a, &fast), (&b, &slow)], 1.0).unwrap();
        assert_eq!(c.dist_m.len(), 1001);
        assert_eq!(c.laps.len(), 2);
        assert!(
            c.laps[0].delta_s.iter().all(|&d| d == 0.0),
            "reference vs itself"
        );
        let d = &c.laps[1].delta_s;
        assert!((d[500] - 2.5).abs() < 1e-3, "12.5 s vs 10 s at 500 m");
        assert!((d[1000] - 5.0).abs() < 1e-3);
        assert_eq!(c.units[names::SPEED], "m/s");
        assert!(c.laps[0].channels.contains_key(names::GEAR));
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(
            resample(&lap(100.0, 10.0), 0.0, None, None),
            Err(AnalysisError::InvalidStep(0.0))
        );
        let empty = LapTelemetry::new(10.0);
        assert_eq!(
            resample(&empty, 1.0, None, None),
            Err(AnalysisError::MissingChannel(names::DIST))
        );
    }

    #[test]
    fn serializes_nan_as_null() {
        let mut c = compare(&[(&meta(1), &lap(100.0, 10.0))], 1.0).unwrap();
        c.laps[0].delta_s[0] = f32::NAN;
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains(r#""deltaS":[null,"#), "{json:.300}");
    }
}
