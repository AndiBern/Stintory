// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The lap recorder: turns [`SourceEvent`]s into sessions and laps.
//!
//! Rules (docs/PLAN.md §5):
//! - A lap ends when the game's lap number increases. The official lap time
//!   from the game wins over our measured time; we wait a few seconds of
//!   session time for it after the lap ended.
//! - A lap that starts in the pits is an **out-lap**, a lap that ends in the
//!   pits an **in-lap**. A lap that starts away from the start/finish line
//!   (joined mid-lap) or is cut off (session end, reset) is **incomplete**.
//! - [`SourceEvent::Invalidate`] marks the current (or a just finished) lap
//!   invalid. If the game's lap number goes back down, the laps it "took
//!   back" are invalid ("not counted by the game").
//! - Session and lap ids are **derived** from the game's session key, lap
//!   number and lap start time, so importing the same data twice produces
//!   the same ids (the store skips duplicates).
//! - [`SourceEvent::Rewind`] removes the rewound samples and flags the lap
//!   as `flashback`. Laps already emitted are not taken back.

use std::collections::VecDeque;

use rz_model::channel::names;
use rz_model::session::LapFlags;
use rz_model::{
    Channel, ChannelData, LapId, LapTelemetry, NewLap, Origin, Session, SessionId, Unit,
};
use time::Duration;

use rz_model::capture::{Frame, SessionMeta, SourceEvent, ValueKind};

/// Tuning of the recorder.
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// Session seconds to wait for the official lap time after a lap ended.
    pub official_time_wait_s: f64,
    /// Laps with fewer samples are dropped (e.g. a blip when entering the car).
    pub min_samples: usize,
    /// A lap whose first sample is further than this from the start/finish
    /// line (and not in the pits) was joined mid-lap and is incomplete.
    pub max_start_dist_m: f32,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            official_time_wait_s: 5.0,
            min_samples: 10,
            max_start_dist_m: 50.0,
        }
    }
}

/// A finished lap: metadata plus telemetry.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedLap {
    /// Lap metadata (blob fields are added when the lap is stored)
    pub lap: NewLap,
    /// The lap's telemetry
    pub telemetry: LapTelemetry,
}

/// What the recorder produced from an event.
#[derive(Debug, Clone, PartialEq)]
pub enum RecorderOutput {
    /// A new session started: store it.
    SessionStarted(Session),
    /// The current session's info changed: update it.
    SessionUpdated(Session),
    /// A lap is finished: store it.
    Lap(Box<RecordedLap>),
    /// The session ended (`ended_at` is set).
    SessionEnded(Session),
}

/// The recorder state machine. Feed events with [`handle`](Self::handle)
/// and call [`finish`](Self::finish) when the source is done.
#[derive(Debug, Default)]
pub struct Recorder {
    config: RecorderConfig,
    active: Option<Active>,
}

#[derive(Debug)]
struct Active {
    session: Session,
    meta: SessionMeta,
    current: Option<LapBuffer>,
    pending: VecDeque<PendingLap>,
    /// Official times that arrived before the lap boundary was seen.
    early_times: Vec<(i32, u32, Vec<u32>)>,
    first_time: Option<f64>,
    last_time: f64,
    /// A rewind discarded the running lap: flag the next lap as flashback.
    flashback_next: bool,
}

#[derive(Debug, Clone)]
struct LapBuffer {
    lap: i32,
    times: Vec<f64>,
    dists: Vec<f32>,
    values: Vec<Vec<f64>>,
    out_lap: bool,
    joined_mid_lap: bool,
    flashback: bool,
    invalid: Vec<String>,
    last_in_pits: bool,
}

#[derive(Debug)]
struct PendingLap {
    buffer: LapBuffer,
    /// Session time of the first sample of the next lap.
    end_time: f64,
    official: Option<(u32, Vec<u32>)>,
    /// The game already told us what it thinks of this lap (e.g. invalid):
    /// no need to wait for an official time.
    resolved: bool,
}

impl LapBuffer {
    fn start(frame: &Frame, config: &RecorderConfig) -> Self {
        Self {
            lap: frame.lap,
            times: Vec::new(),
            dists: Vec::new(),
            values: Vec::new(),
            out_lap: frame.in_pits,
            joined_mid_lap: !frame.in_pits && frame.lap_dist > config.max_start_dist_m,
            flashback: false,
            invalid: Vec::new(),
            last_in_pits: frame.in_pits,
        }
    }

    fn push(&mut self, frame: Frame) {
        self.times.push(frame.session_time);
        self.dists.push(frame.lap_dist);
        self.last_in_pits = frame.in_pits;
        self.values.push(frame.values);
    }

    fn start_time(&self) -> Option<f64> {
        self.times.first().copied()
    }

    /// Removes all samples at or after `t`.
    fn truncate_from(&mut self, t: f64) {
        let keep = self.times.partition_point(|&x| x < t);
        self.times.truncate(keep);
        self.dists.truncate(keep);
        self.values.truncate(keep);
        self.flashback = true;
    }
}

impl Recorder {
    /// Creates a recorder with the given configuration.
    pub fn new(config: RecorderConfig) -> Self {
        Self {
            config,
            active: None,
        }
    }

    /// The currently active session, if any.
    pub fn session(&self) -> Option<&Session> {
        self.active.as_ref().map(|a| &a.session)
    }

    /// Processes one event.
    pub fn handle(&mut self, event: SourceEvent) -> Vec<RecorderOutput> {
        let mut out = Vec::new();
        match event {
            SourceEvent::Session(meta) => self.on_session(meta, &mut out),
            SourceEvent::Frame(frame) => self.on_frame(frame, &mut out),
            SourceEvent::LapTime {
                lap,
                lap_time_ms,
                sector_times_ms,
            } => self.on_lap_time(lap, lap_time_ms, sector_times_ms, &mut out),
            SourceEvent::Invalidate { lap, reason } => self.on_invalidate(lap, reason, &mut out),
            SourceEvent::Rewind { to_session_time } => self.on_rewind(to_session_time),
            SourceEvent::LapAborted { reason } => self.on_lap_aborted(reason, &mut out),
            SourceEvent::SessionEnd => self.end_session(&mut out),
        }
        out
    }

    /// Ends the active session (end of file, shutdown).
    pub fn finish(&mut self) -> Vec<RecorderOutput> {
        let mut out = Vec::new();
        self.end_session(&mut out);
        out
    }

    fn on_session(&mut self, meta: SessionMeta, out: &mut Vec<RecorderOutput>) {
        if let Some(active) = self.active.as_mut()
            && active.meta.session_key == meta.session_key
        {
            if active.meta != meta {
                let channels_changed = active.meta.channels != meta.channels;
                if channels_changed {
                    // Frames of the running lap don't match the new layout anymore.
                    if let Some(cur) = active.current.take() {
                        let lap = self.build_lap_from(cur, None, true);
                        if let Some(lap) = lap {
                            out.push(RecorderOutput::Lap(Box::new(lap)));
                        }
                    }
                }
                let active = self.active.as_mut().expect("checked above");
                apply_meta(&mut active.session, &meta);
                active.meta = meta;
                out.push(RecorderOutput::SessionUpdated(active.session.clone()));
            }
            return;
        }
        self.end_session(out);
        let mut session = Session {
            id: SessionId::derived(&meta.session_key),
            game: meta.game,
            game_version: None,
            track_key: String::new(),
            car_key: String::new(),
            session_type: meta.session_type,
            started_at: meta.started_at,
            ended_at: None,
            conditions: Default::default(),
            setup: None,
            session_info: None,
            origin: Origin::Own,
        };
        apply_meta(&mut session, &meta);
        out.push(RecorderOutput::SessionStarted(session.clone()));
        self.active = Some(Active {
            session,
            meta,
            current: None,
            pending: VecDeque::new(),
            early_times: Vec::new(),
            first_time: None,
            last_time: 0.0,
            flashback_next: false,
        });
    }

    fn on_frame(&mut self, frame: Frame, out: &mut Vec<RecorderOutput>) {
        let config = self.config.clone();
        let Some(active) = self.active.as_mut() else {
            tracing::trace!("frame without session ignored");
            return;
        };
        if frame.values.len() != active.meta.channels.len() {
            tracing::warn!(
                expected = active.meta.channels.len(),
                actual = frame.values.len(),
                "frame with wrong number of values ignored"
            );
            return;
        }
        active.first_time.get_or_insert(frame.session_time);
        active.last_time = frame.session_time;

        match active.current.as_mut() {
            None => {
                let mut buf = LapBuffer::start(&frame, &config);
                buf.flashback = std::mem::take(&mut active.flashback_next);
                buf.push(frame);
                active.current = Some(buf);
            }
            Some(cur) if frame.lap == cur.lap => cur.push(frame),
            Some(cur) if frame.lap > cur.lap => {
                // Lap boundary: the current lap is complete.
                let finished = std::mem::replace(cur, LapBuffer::start(&frame, &config));
                let end_time = frame.session_time;
                cur.push(frame);
                let official = take_early_time(&mut active.early_times, finished.lap);
                active.pending.push_back(PendingLap {
                    buffer: finished,
                    end_time,
                    official,
                    resolved: false,
                });
            }
            Some(_) => {
                // Lap number went down without a rewind: the game didn't
                // count the last line crossing(s), or the car was reset.
                for p in active
                    .pending
                    .iter_mut()
                    .filter(|p| p.buffer.lap >= frame.lap)
                {
                    push_reason(&mut p.buffer.invalid, NOT_COUNTED);
                    p.resolved = true;
                }
                let old = active.current.take().expect("matched Some");
                let mut buf = LapBuffer::start(&frame, &config);
                buf.push(frame);
                active.current = Some(buf);
                // emit the taken-back laps first, so laps come out in order
                self.flush_pending(false, out);
                if let Some(lap) = self.build_lap_from(old, None, true) {
                    out.push(RecorderOutput::Lap(Box::new(lap)));
                }
            }
        }
        self.flush_pending(false, out);
    }

    fn on_lap_time(
        &mut self,
        lap: i32,
        lap_time_ms: u32,
        sector_times_ms: Vec<u32>,
        out: &mut Vec<RecorderOutput>,
    ) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if let Some(p) = active.pending.iter_mut().find(|p| p.buffer.lap == lap) {
            p.official = Some((lap_time_ms, sector_times_ms));
        } else if active.current.as_ref().is_some_and(|c| c.lap == lap) {
            active.early_times.push((lap, lap_time_ms, sector_times_ms));
        } else {
            tracing::debug!(lap, "official time for unknown lap ignored");
        }
        self.flush_pending(false, out);
    }

    fn on_rewind(&mut self, to: f64) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        active.last_time = to;
        // Rewinding into a pending lap re-opens it.
        let rewinds_into_pending = |a: &Active| {
            a.current
                .as_ref()
                .and_then(LapBuffer::start_time)
                .is_none_or(|start| to < start)
                && a.pending.back().is_some()
        };
        if rewinds_into_pending(active) {
            let reopened = active.pending.pop_back().expect("checked");
            active.current = Some(reopened.buffer);
        }
        if let Some(cur) = active.current.as_mut() {
            cur.truncate_from(to);
            if cur.times.is_empty() {
                // Rewound to before this lap started; the next frame starts a new buffer.
                active.current = None;
                active.flashback_next = true;
            }
        }
    }

    fn on_invalidate(&mut self, lap: Option<i32>, reason: String, out: &mut Vec<RecorderOutput>) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        match lap {
            None => {
                if let Some(cur) = active.current.as_mut() {
                    push_reason(&mut cur.invalid, &reason);
                }
            }
            Some(lap) => {
                if let Some(p) = active.pending.iter_mut().find(|p| p.buffer.lap == lap) {
                    push_reason(&mut p.buffer.invalid, &reason);
                    p.resolved = true;
                } else if let Some(cur) = active.current.as_mut().filter(|c| c.lap == lap) {
                    push_reason(&mut cur.invalid, &reason);
                } else {
                    tracing::debug!(lap, "invalidation for unknown lap ignored");
                }
            }
        }
        self.flush_pending(false, out);
    }

    fn on_lap_aborted(&mut self, reason: String, out: &mut Vec<RecorderOutput>) {
        let Some(mut cur) = self.active.as_mut().and_then(|a| a.current.take()) else {
            return;
        };
        push_reason(&mut cur.invalid, &reason);
        if let Some(lap) = self.build_lap_from(cur, None, true) {
            out.push(RecorderOutput::Lap(Box::new(lap)));
        }
    }

    fn end_session(&mut self, out: &mut Vec<RecorderOutput>) {
        if self.active.is_none() {
            return;
        }
        self.flush_pending(true, out);
        let mut active = self.active.take().expect("checked");
        if let Some(cur) = active.current.take() {
            let session_id = active.session.id;
            if let Some(lap) = build_lap(&self.config, session_id, &active.meta, cur, None, true) {
                out.push(RecorderOutput::Lap(Box::new(lap)));
            }
        }
        let duration = active.last_time - active.first_time.unwrap_or(active.last_time);
        active.session.ended_at =
            Some(active.session.started_at + Duration::seconds_f64(duration.max(0.0)));
        out.push(RecorderOutput::SessionEnded(active.session));
    }

    /// Emits pending laps whose official time arrived or whose wait expired.
    /// With `force`, all pending laps are emitted.
    fn flush_pending(&mut self, force: bool, out: &mut Vec<RecorderOutput>) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        while let Some(front) = active.pending.front() {
            let expired = active.last_time - front.end_time >= self.config.official_time_wait_s;
            if !(force || front.official.is_some() || front.resolved || expired) {
                break;
            }
            let p = active.pending.pop_front().expect("front exists");
            let measured = p.buffer.start_time().map(|s| p.end_time - s);
            let lap = build_lap(
                &self.config,
                active.session.id,
                &active.meta,
                p.buffer,
                p.official.map(|o| (Some(o.0), o.1)).or(Some((
                    measured.map(|m| (m * 1000.0).round() as u32),
                    Vec::new(),
                ))),
                false,
            );
            if let Some(lap) = lap {
                out.push(RecorderOutput::Lap(Box::new(lap)));
            }
        }
    }

    fn build_lap_from(
        &self,
        buffer: LapBuffer,
        time: Option<(Option<u32>, Vec<u32>)>,
        cut_off: bool,
    ) -> Option<RecordedLap> {
        let active = self.active.as_ref()?;
        build_lap(
            &self.config,
            active.session.id,
            &active.meta,
            buffer,
            time,
            cut_off,
        )
    }
}

const NOT_COUNTED: &str = "not counted by the game";

fn push_reason(reasons: &mut Vec<String>, reason: &str) {
    if !reasons.iter().any(|r| r == reason) {
        reasons.push(reason.to_owned());
    }
}

fn take_early_time(early: &mut Vec<(i32, u32, Vec<u32>)>, lap: i32) -> Option<(u32, Vec<u32>)> {
    let idx = early.iter().position(|(l, _, _)| *l == lap)?;
    let (_, ms, sectors) = early.remove(idx);
    Some((ms, sectors))
}

fn apply_meta(session: &mut Session, meta: &SessionMeta) {
    session.game = meta.game;
    session.game_version.clone_from(&meta.game_version);
    session.track_key.clone_from(&meta.track_key);
    session.car_key.clone_from(&meta.car_key);
    session.session_type = meta.session_type;
    session.conditions = meta.conditions.clone();
    session.setup.clone_from(&meta.setup);
    session.session_info.clone_from(&meta.session_info);
}

/// Builds the lap. `time` is `(lap_time, sectors)`; `cut_off` marks laps that
/// ended without crossing the line.
fn build_lap(
    config: &RecorderConfig,
    session_id: SessionId,
    meta: &SessionMeta,
    buffer: LapBuffer,
    time: Option<(Option<u32>, Vec<u32>)>,
    cut_off: bool,
) -> Option<RecordedLap> {
    if buffer.times.len() < config.min_samples {
        tracing::debug!(
            lap = buffer.lap,
            samples = buffer.times.len(),
            "lap too short, dropped"
        );
        return None;
    }
    let incomplete = cut_off || buffer.joined_mid_lap;
    let flags = LapFlags {
        out_lap: buffer.out_lap,
        in_lap: !cut_off && buffer.last_in_pits,
        incomplete,
        flashback: buffer.flashback,
    };
    let (lap_time_ms, sector_times_ms) = if incomplete {
        (None, Vec::new())
    } else {
        time.unwrap_or((None, Vec::new()))
    };
    let mut reasons = buffer.invalid.clone();
    if incomplete {
        reasons.insert(0, "incomplete".to_owned());
    }
    let valid = reasons.is_empty();

    let telemetry = match build_telemetry(meta, &buffer) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "could not build lap telemetry");
            return None;
        }
    };
    Some(RecordedLap {
        lap: NewLap {
            id: lap_id(meta, &buffer),
            session_id,
            number: u32::try_from(buffer.lap.max(0)).unwrap_or(0),
            lap_time_ms,
            sector_times_ms,
            valid,
            invalid_reason: (!valid).then(|| reasons.join(", ")),
            flags,
        },
        telemetry,
    })
}

/// Stable lap id: session key + lap number + lap start (0.1 s resolution).
fn lap_id(meta: &SessionMeta, buffer: &LapBuffer) -> LapId {
    let start_ds = (buffer.start_time().unwrap_or(0.0) * 10.0).round() as i64;
    LapId::derived(&format!("{}/{}/{}", meta.session_key, buffer.lap, start_ds))
}

fn build_telemetry(
    meta: &SessionMeta,
    buffer: &LapBuffer,
) -> Result<LapTelemetry, rz_model::TelemetryError> {
    let start = buffer.start_time().unwrap_or(0.0);
    let mut telemetry = LapTelemetry::new(meta.sample_rate_hz);
    telemetry.add_channel(Channel::new(
        names::TIME,
        Unit::Second,
        ChannelData::F32(buffer.times.iter().map(|t| (t - start) as f32).collect()),
    ))?;
    telemetry.add_channel(Channel::new(
        names::DIST,
        Unit::Meter,
        ChannelData::F32(buffer.dists.clone()),
    ))?;
    for (i, spec) in meta.channels.iter().enumerate() {
        let column = buffer.values.iter().map(|v| v[i]);
        let data = match spec.kind {
            ValueKind::F32 => ChannelData::F32(column.map(|x| x as f32).collect()),
            ValueKind::F64 => ChannelData::F64(column.collect()),
            ValueKind::I32 => ChannelData::I32(column.map(|x| x.round() as i32).collect()),
            ValueKind::Bool => ChannelData::Bool(column.map(|x| x != 0.0).collect()),
        };
        telemetry.add_channel(Channel::new(spec.name.clone(), spec.unit, data))?;
    }
    Ok(telemetry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rz_model::capture::ChannelSpec;
    use rz_model::{GameId, SessionType};
    use time::macros::datetime;

    const TRACK_M: f32 = 1000.0;
    const HZ: f64 = 10.0;

    fn meta(key: &str) -> SessionMeta {
        SessionMeta {
            session_key: key.into(),
            game: GameId::Iracing,
            game_version: None,
            track_key: "iracing:1:test".into(),
            car_key: "iracing:1".into(),
            session_type: SessionType::Practice,
            started_at: datetime!(2026-11-10 18:00 UTC),
            conditions: Default::default(),
            setup: None,
            session_info: None,
            sample_rate_hz: HZ as f32,
            channels: vec![
                ChannelSpec::new(names::SPEED, Unit::MeterPerSecond, ValueKind::F32),
                ChannelSpec::new(names::GEAR, Unit::None, ValueKind::I32),
            ],
        }
    }

    /// Drives with a feeder that tracks session time.
    struct Drive {
        rec: Recorder,
        t: f64,
        out: Vec<RecorderOutput>,
    }

    impl Drive {
        fn new() -> Self {
            let mut d = Self {
                rec: Recorder::new(RecorderConfig::default()),
                t: 100.0,
                out: Vec::new(),
            };
            d.send(SourceEvent::Session(meta("s1")));
            d
        }

        fn send(&mut self, e: SourceEvent) {
            self.out.extend(self.rec.handle(e));
        }

        fn frame(&mut self, lap: i32, dist: f32, in_pits: bool) {
            let f = Frame {
                session_time: self.t,
                lap,
                lap_dist: dist,
                in_pits,
                values: vec![50.0, 3.0],
            };
            self.t += 1.0 / HZ;
            self.send(SourceEvent::Frame(f));
        }

        /// Drives from `from` m to the end of lap `lap` (20 s per lap).
        fn drive_lap(&mut self, lap: i32, from: f32, pits_until: f32) {
            let samples = 200;
            for i in 0..samples {
                let dist = from + (TRACK_M - from) * i as f32 / samples as f32;
                self.frame(lap, dist, dist < pits_until);
            }
        }

        fn laps(&self) -> Vec<&RecordedLap> {
            self.out
                .iter()
                .filter_map(|o| match o {
                    RecorderOutput::Lap(l) => Some(l.as_ref()),
                    _ => None,
                })
                .collect()
        }
    }

    #[test]
    fn records_laps_with_official_times() {
        let mut d = Drive::new();
        assert!(matches!(d.out[0], RecorderOutput::SessionStarted(_)));
        d.drive_lap(1, 0.0, 0.0);
        for i in 0..10 {
            d.frame(2, i as f32, false);
        }
        assert!(d.laps().is_empty(), "waits for the official time");
        // Official time of lap 1 arrives shortly after crossing the line.
        d.send(SourceEvent::LapTime {
            lap: 1,
            lap_time_ms: 19_987,
            sector_times_ms: vec![6000, 7000, 6987],
        });
        let laps = d.laps();
        assert_eq!(laps.len(), 1);
        assert_eq!(laps[0].lap.number, 1);
        assert_eq!(laps[0].lap.lap_time_ms, Some(19_987));
        assert_eq!(laps[0].lap.sector_times_ms, vec![6000, 7000, 6987]);
        assert!(laps[0].lap.valid);
        assert_eq!(laps[0].telemetry.len(), 200);
        let t = laps[0].telemetry.channel(names::TIME).unwrap();
        assert!(matches!(&t.data, ChannelData::F32(v) if v[0] == 0.0));
        assert!(laps[0].telemetry.channel(names::GEAR).is_some());
    }

    #[test]
    fn early_official_time_is_used() {
        let mut d = Drive::new();
        d.drive_lap(1, 0.0, 0.0);
        d.send(SourceEvent::LapTime {
            lap: 1,
            lap_time_ms: 20_001,
            sector_times_ms: vec![],
        });
        d.frame(2, 1.0, false);
        assert_eq!(d.laps()[0].lap.lap_time_ms, Some(20_001));
    }

    #[test]
    fn falls_back_to_measured_time() {
        let mut d = Drive::new();
        d.drive_lap(1, 0.0, 0.0);
        d.drive_lap(2, 0.0, 0.0); // 20 s later, wait (5 s) expired
        let laps = d.laps();
        assert_eq!(laps.len(), 1);
        assert_eq!(laps[0].lap.lap_time_ms, Some(20_000));
    }

    #[test]
    fn out_lap_in_lap_and_session_end() {
        let mut d = Drive::new();
        d.drive_lap(1, 0.0, 300.0); // leaves the pits
        d.drive_lap(2, 0.0, 0.0);
        // lap 3 ends in the pits, then the session ends mid-lap 4
        for i in 0..200 {
            let dist = TRACK_M * i as f32 / 200.0;
            d.frame(3, dist, dist > 900.0);
        }
        for i in 0..50 {
            d.frame(4, i as f32, true);
        }
        d.send(SourceEvent::SessionEnd);

        let laps = d.laps();
        assert_eq!(laps.len(), 4);
        assert!(laps[0].lap.flags.out_lap);
        assert!(!laps[1].lap.flags.out_lap && !laps[1].lap.flags.in_lap);
        assert!(laps[2].lap.flags.in_lap);
        assert!(laps[3].lap.flags.incomplete);
        assert_eq!(laps[3].lap.lap_time_ms, None);
        assert!(!laps[3].lap.valid);
        let RecorderOutput::SessionEnded(s) = d.out.last().unwrap() else {
            panic!("session must end last");
        };
        assert_eq!(
            s.ended_at,
            Some(datetime!(2026-11-10 18:00 UTC) + Duration::seconds_f64(64.9))
        );
    }

    #[test]
    fn joined_mid_lap_is_incomplete() {
        let mut d = Drive::new();
        d.drive_lap(1, 400.0, 0.0);
        d.drive_lap(2, 0.0, 0.0);
        let laps = d.laps();
        assert!(laps[0].lap.flags.incomplete);
        assert_eq!(laps[0].lap.invalid_reason.as_deref(), Some("incomplete"));
    }

    #[test]
    fn invalidation_marks_current_lap_only() {
        let mut d = Drive::new();
        d.drive_lap(1, 0.0, 0.0);
        d.send(SourceEvent::Invalidate {
            lap: None,
            reason: "off track".into(),
        });
        d.send(SourceEvent::Invalidate {
            lap: None,
            reason: "off track".into(),
        });
        d.drive_lap(2, 0.0, 0.0);
        d.drive_lap(3, 0.0, 0.0);
        let laps = d.laps();
        assert!(!laps[0].lap.valid);
        assert_eq!(laps[0].lap.invalid_reason.as_deref(), Some("off track"));
        assert!(laps[1].lap.valid);
    }

    #[test]
    fn rewind_truncates_and_flags_flashback() {
        let mut d = Drive::new();
        d.drive_lap(1, 0.0, 0.0);
        // drive half of lap 2, then flash back 5 s
        for i in 0..100 {
            d.frame(2, i as f32 * 5.0, false);
        }
        let rewind_to = d.t - 5.0;
        d.send(SourceEvent::Rewind {
            to_session_time: rewind_to,
        });
        d.t = rewind_to;
        for i in 50..200 {
            d.frame(2, i as f32 * 5.0, false);
        }
        d.drive_lap(3, 0.0, 0.0);
        let laps = d.laps();
        assert_eq!(laps.len(), 2);
        assert!(!laps[0].lap.flags.flashback);
        assert!(laps[1].lap.flags.flashback);
        assert_eq!(
            laps[1].telemetry.len(),
            200,
            "rewound samples were replaced"
        );
        let ChannelData::F32(t) = &laps[1].telemetry.channel(names::TIME).unwrap().data else {
            panic!()
        };
        assert!(t.windows(2).all(|w| w[1] > w[0]), "time is monotonic");
    }

    #[test]
    fn new_session_key_ends_previous_session() {
        let mut d = Drive::new();
        d.drive_lap(1, 0.0, 0.0);
        d.send(SourceEvent::Session(meta("s2")));
        let kinds: Vec<_> = d
            .out
            .iter()
            .map(|o| match o {
                RecorderOutput::SessionStarted(_) => "start",
                RecorderOutput::SessionUpdated(_) => "update",
                RecorderOutput::Lap(_) => "lap",
                RecorderOutput::SessionEnded(_) => "end",
            })
            .collect();
        assert_eq!(kinds, ["start", "lap", "end", "start"]);
    }

    #[test]
    fn same_session_key_updates_session() {
        let mut d = Drive::new();
        let mut m = meta("s1");
        m.conditions.track_temp_c = Some(31.0);
        d.send(SourceEvent::Session(m.clone()));
        d.send(SourceEvent::Session(m)); // unchanged: no output
        let updates: Vec<_> = d
            .out
            .iter()
            .filter_map(|o| match o {
                RecorderOutput::SessionUpdated(s) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].conditions.track_temp_c, Some(31.0));
    }

    #[test]
    fn aborted_lap_is_incomplete_and_next_lap_starts_fresh() {
        let mut d = Drive::new();
        d.drive_lap(1, 0.0, 0.0);
        for i in 0..100 {
            d.frame(2, i as f32 * 5.0, false);
        }
        d.send(SourceEvent::LapAborted {
            reason: "towed".into(),
        });
        // back in the pit box, same lap number
        d.drive_lap(2, 100.0, 300.0);
        d.drive_lap(3, 0.0, 0.0);
        let laps = d.laps();
        assert_eq!(laps.len(), 3);
        assert!(laps[1].lap.flags.incomplete);
        assert_eq!(
            laps[1].lap.invalid_reason.as_deref(),
            Some("incomplete, towed")
        );
        assert!(
            laps[2].lap.flags.out_lap,
            "restart from the pits is an out-lap"
        );
    }

    #[test]
    fn game_can_invalidate_a_finished_lap() {
        let mut d = Drive::new();
        d.drive_lap(1, 0.0, 0.0);
        d.frame(2, 1.0, false);
        d.send(SourceEvent::Invalidate {
            lap: Some(1),
            reason: "invalidated by game".into(),
        });
        let laps = d.laps();
        assert_eq!(laps.len(), 1, "emitted right away, no need to wait");
        assert!(!laps[0].lap.valid);
        assert_eq!(laps[0].lap.lap_time_ms, Some(20_000), "measured time kept");
    }

    #[test]
    fn lap_taken_back_by_the_game_is_not_counted() {
        let mut d = Drive::new();
        d.drive_lap(3, 0.0, 0.0);
        for i in 0..10 {
            d.frame(4, i as f32, false); // crossed the line ...
        }
        d.frame(3, 130.0, false); // ... but the game says we're still on lap 3
        let laps = d.laps();
        assert_eq!(laps[0].lap.number, 3);
        assert_eq!(laps[0].lap.invalid_reason.as_deref(), Some(NOT_COUNTED));
    }

    #[test]
    fn crossing_the_line_in_the_pit_lane_starts_an_out_lap() {
        let mut d = Drive::new();
        d.drive_lap(1, 0.0, 0.0);
        // in-lap: lap 2 ends in the pit lane, lap 3 starts there
        for i in 0..200 {
            let dist = TRACK_M * i as f32 / 200.0;
            d.frame(2, dist, dist > 900.0);
        }
        d.drive_lap(3, -1.0, 300.0);
        d.drive_lap(4, 0.0, 0.0);
        let laps = d.laps();
        assert!(laps[1].lap.flags.in_lap);
        assert!(laps[2].lap.flags.out_lap);
        assert!(!laps[2].lap.flags.incomplete);
    }

    #[test]
    fn ids_are_deterministic() {
        let run = || {
            let mut d = Drive::new();
            d.drive_lap(1, 0.0, 0.0);
            d.drive_lap(2, 0.0, 0.0);
            d.send(SourceEvent::SessionEnd);
            d.out
                .iter()
                .filter_map(|o| match o {
                    RecorderOutput::Lap(l) => Some((l.lap.session_id, l.lap.id)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let a = run();
        assert_eq!(a, run());
        assert_eq!(a[0].0, SessionId::derived("s1"));
        assert_ne!(a[0].1, a[1].1);
    }

    #[test]
    fn short_blips_are_dropped() {
        let mut d = Drive::new();
        for i in 0..3 {
            d.frame(1, i as f32, true);
        }
        d.send(SourceEvent::SessionEnd);
        assert!(d.laps().is_empty());
    }

    #[test]
    fn frames_with_wrong_value_count_are_ignored() {
        let mut d = Drive::new();
        d.send(SourceEvent::Frame(Frame {
            session_time: 1.0,
            lap: 1,
            lap_dist: 0.0,
            in_pits: false,
            values: vec![1.0],
        }));
        assert!(d.rec.active.as_ref().unwrap().current.is_none());
    }
}
