// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The MCP tools. They call the same handlers as the web UI, so access is
//! checked the same way: the user's own data and laps others shared with
//! them. Answers are compact JSON for a language model: rounded numbers,
//! speeds in km/h, pedals in percent, empty fields left out.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde_json::{Map, Value, json};
use sy_model::api::{CompareData, CompareRequest, CornerLapStats, ReferenceScope, SessionSummary};
use sy_model::channel::names;
use sy_model::{LapId, SessionType};
use uuid::Uuid;

use crate::auth::{Auth, Session};
use crate::error::ApiError;
use crate::sharing::ReferenceQuery;
use crate::state::SharedState;

/// Most grid points `lap_telemetry` returns per channel.
const MAX_POINTS: usize = 600;

/// The tool definitions for `tools/list` (fixed order).
pub fn definitions() -> Vec<Value> {
    let read_only = json!({ "readOnlyHint": true, "openWorldHint": false });
    let tool = |name: &str, title: &str, description: &str, schema: Value| {
        json!({
            "name": name,
            "title": title,
            "description": description,
            "inputSchema": schema,
            "annotations": read_only,
        })
    };
    let lap_id =
        json!({ "type": "string", "description": "Lap id (from get_session or best_laps)" });
    vec![
        tool(
            "list_tracks",
            "Tracks and cars",
            "Every track and car combination the user has driven, with the number of laps, \
             the user's best lap and the best lap others shared with them. Start here to see \
             what data exists.",
            json!({ "type": "object", "additionalProperties": false }),
        ),
        tool(
            "list_sessions",
            "Sessions",
            "The user's sessions, newest first: track, car, session type, date, number of laps \
             and best lap. Filter by part of the track or car name.",
            json!({
                "type": "object",
                "properties": {
                    "track": { "type": "string", "description": "Part of the track name or key" },
                    "car": { "type": "string", "description": "Part of the car name or key" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
                },
                "additionalProperties": false
            }),
        ),
        tool(
            "get_session",
            "Session laps",
            "The laps of one of the user's sessions: lap and sector times, validity (and why a \
             lap is invalid), out/in laps, weather, fuel and tyres per lap.",
            json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Session id (from list_sessions)" }
                },
                "required": ["session_id"],
                "additionalProperties": false
            }),
        ),
        tool(
            "best_laps",
            "Best laps for a track and car",
            "The fastest clean laps for a track and car, fastest first: the user's own \
             (recorded and imported) and laps others shared with them. Use one as the \
             reference for compare_laps.",
            json!({
                "type": "object",
                "properties": {
                    "track_key": { "type": "string", "description": "Track key (from list_tracks or list_sessions)" },
                    "car_key": { "type": "string", "description": "Car key" },
                    "scope": { "type": "string", "enum": ["all", "mine", "shared"], "default": "all" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10 }
                },
                "required": ["track_key", "car_key"],
                "additionalProperties": false
            }),
        ),
        tool(
            "compare_laps",
            "Compare a lap with a reference",
            "Corner by corner analysis of a lap against a reference lap of the same track and \
             car: time lost per corner (split into entry and exit), and for both laps braking \
             point and pressure, lift, apex speed and gear, throttle pick-up, full throttle and \
             exit speed. The corners with the most time lost are listed first in biggestLosses.",
            json!({
                "type": "object",
                "properties": {
                    "lap_id": lap_id,
                    "reference_lap_id": { "type": "string", "description": "The faster lap to compare with" }
                },
                "required": ["lap_id", "reference_lap_id"],
                "additionalProperties": false
            }),
        ),
        tool(
            "lap_telemetry",
            "Telemetry of a part of a lap",
            "Telemetry traces on a distance grid for a part of a lap, optionally next to a \
             reference lap with the running time difference. For a close look at a corner \
             (use its segment from compare_laps). Channels: speed (km/h), throttle and brake \
             (%), gear, steer (degrees), rpm, g_lat and g_long (m/s²), abs_active, tc_active.",
            json!({
                "type": "object",
                "properties": {
                    "lap_id": lap_id,
                    "reference_lap_id": { "type": "string", "description": "Optional lap to compare with" },
                    "from_m": { "type": "number", "minimum": 0, "description": "Start, metres from the line" },
                    "to_m": { "type": "number", "minimum": 0, "description": "End, metres from the line" },
                    "step_m": { "type": "number", "minimum": 1, "maximum": 100, "default": 5 },
                    "channels": {
                        "type": "array",
                        "items": { "type": "string" },
                        "default": ["speed", "throttle", "brake", "gear", "steer"]
                    }
                },
                "required": ["lap_id"],
                "additionalProperties": false
            }),
        ),
    ]
}

/// Runs a tool: Some(Ok(text)), Some(Err(message)) when the tool fails (for
/// the model to correct its call), None for an unknown tool.
pub async fn call(
    state: &SharedState,
    session: &Session,
    name: &str,
    args: &Value,
) -> Option<Result<String, String>> {
    let ctx = Ctx { state, session };
    let result = match name {
        "list_tracks" => ctx.list_tracks().await,
        "list_sessions" => ctx.list_sessions(args).await,
        "get_session" => ctx.get_session(args).await,
        "best_laps" => ctx.best_laps(args).await,
        "compare_laps" => ctx.compare_laps(args).await,
        "lap_telemetry" => ctx.lap_telemetry(args).await,
        _ => return None,
    };
    Some(result.map(|v| serde_json::to_string(&compact(v)).unwrap_or_default()))
}

struct Ctx<'a> {
    state: &'a SharedState,
    session: &'a Session,
}

type ToolResult = Result<Value, String>;

/// An API error as a message for the model (details of server errors stay
/// in the log).
fn api_error(e: ApiError) -> String {
    if e.status().is_server_error() {
        tracing::error!(error = %e.message(), "MCP tool failed");
        "internal error".into()
    } else {
        e.message().to_owned()
    }
}

fn arg_str<'v>(args: &'v Value, key: &str) -> Option<&'v str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn required_str<'v>(args: &'v Value, key: &str) -> Result<&'v str, String> {
    arg_str(args, key).ok_or_else(|| format!("missing argument {key}"))
}

fn arg_uuid(args: &Value, key: &str) -> Result<Uuid, String> {
    required_str(args, key)?
        .parse()
        .map_err(|_| format!("{key} is not a valid id"))
}

fn arg_limit(args: &Value, default: usize, max: usize) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .map_or(default, |n| (n as usize).clamp(1, max))
}

/// `m:ss.mmm`
fn lap_time(ms: Option<u32>) -> Value {
    match ms {
        Some(ms) => json!(format!(
            "{}:{:02}.{:03}",
            ms / 60_000,
            ms / 1000 % 60,
            ms % 1000
        )),
        None => Value::Null,
    }
}

fn round(v: f32, digits: i32) -> Value {
    if !v.is_finite() {
        return Value::Null;
    }
    if digits <= 0 {
        return json!(f64::from(v).round() as i64);
    }
    let f = 10f64.powi(digits);
    json!((f64::from(v) * f).round() / f)
}

fn opt(v: Option<f32>, digits: i32) -> Value {
    v.map_or(Value::Null, |v| round(v, digits))
}

fn kmh(v: Option<f32>) -> Value {
    opt(v.map(|v| v * 3.6), 1)
}

fn session_type(t: SessionType) -> &'static str {
    match t {
        SessionType::Practice => "practice",
        SessionType::Qualifying => "qualifying",
        SessionType::Race => "race",
        SessionType::TimeTrial => "time_trial",
        SessionType::Other => "other",
    }
}

/// Removes nulls, empty strings, empty arrays and empty objects.
fn compact(v: Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, compact(v)))
                .filter(|(_, v)| !is_empty(v))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(compact).collect()),
        v => v,
    }
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

fn session_json(s: &SessionSummary) -> Value {
    json!({
        "sessionId": s.id.to_string(),
        "game": s.game,
        "track": s.track_name,
        "trackKey": s.track_key,
        "car": s.car_name,
        "carKey": s.car_key,
        "type": session_type(s.session_type),
        "startedAt": s.started_at.date().to_string(),
        "laps": s.lap_count,
        "bestLap": lap_time(s.best_lap_ms),
    })
}

fn stats_json(s: &CornerLapStats) -> Value {
    let m = |v: Option<f32>| opt(v, 0);
    json!({
        "timeS": opt(s.time_s, 3),
        "topSpeedKmh": kmh(s.top_speed_mps),
        "liftM": m(s.lift_m),
        "brakeM": m(s.brake_m),
        "brakeMaxPct": opt(s.brake_max.map(|b| b * 100.0), 0),
        "brakeReleaseM": m(s.brake_release_m),
        "minSpeedKmh": kmh(s.min_speed_mps),
        "minSpeedM": m(s.min_speed_m),
        "gear": s.min_speed_gear,
        "throttleM": m(s.throttle_m),
        "fullThrottleM": m(s.full_throttle_m),
        "exitSpeedKmh": kmh(s.exit_speed_mps),
        "flatOut": if s.flat_out { json!(true) } else { Value::Null },
    })
}

impl Ctx<'_> {
    fn auth(&self) -> Auth {
        Auth(self.session.clone())
    }

    async fn sessions(&self) -> Result<Vec<SessionSummary>, String> {
        crate::data::list_sessions(State(self.state.clone()), self.auth())
            .await
            .map(|Json(s)| s)
            .map_err(api_error)
    }

    async fn compare(&self, lap_ids: Vec<Uuid>, step_m: f32) -> Result<CompareData, String> {
        let request = CompareRequest {
            lap_ids: lap_ids.into_iter().map(LapId::from_uuid).collect(),
            step_m: Some(step_m),
        };
        crate::data::compare(State(self.state.clone()), self.auth(), Json(request))
            .await
            .map(|Json(d)| d)
            .map_err(api_error)
    }

    async fn list_tracks(&self) -> ToolResult {
        // (track key, car key) → (track, car, sessions, laps, best, latest)
        let mut combos: BTreeMap<(String, String), Value> = BTreeMap::new();
        let mut latest: BTreeMap<(String, String), String> = BTreeMap::new();
        for s in self.sessions().await? {
            let key = (s.track_key.clone(), s.car_key.clone());
            let entry = combos.entry(key.clone()).or_insert_with(|| {
                json!({
                    "track": s.track_name, "trackKey": s.track_key,
                    "car": s.car_name, "carKey": s.car_key, "game": s.game,
                    "sessions": 0, "laps": 0, "bestLapMs": null,
                })
            });
            entry["sessions"] = json!(entry["sessions"].as_u64().unwrap_or(0) + 1);
            entry["laps"] = json!(entry["laps"].as_u64().unwrap_or(0) + u64::from(s.lap_count));
            if let Some(best) = s.best_lap_ms {
                let current = entry["bestLapMs"].as_u64();
                if current.is_none_or(|c| u64::from(best) < c) {
                    entry["bestLapMs"] = json!(best);
                }
            }
            let date = s.started_at.date().to_string();
            let l = latest.entry(key).or_default();
            if date > *l {
                *l = date;
            }
        }
        let shared = crate::sharing::shared_tracks(State(self.state.clone()), self.auth())
            .await
            .map(|Json(t)| t)
            .map_err(api_error)?;
        let mut shared_only = Vec::new();
        for t in shared {
            let key = (t.track_key.clone(), t.car_key.clone());
            let driver = t.best_driver.as_ref().map(|d| d.username.clone());
            match combos.get_mut(&key) {
                Some(entry) => {
                    entry["othersBestLap"] = lap_time(t.best_lap_ms);
                    entry["othersBestDriver"] = json!(driver);
                }
                None => shared_only.push(json!({
                    "track": t.track_name, "trackKey": t.track_key,
                    "car": t.car_name, "carKey": t.car_key, "game": t.game,
                    "sharedLaps": t.laps, "othersBestLap": lap_time(t.best_lap_ms),
                    "othersBestDriver": driver,
                })),
            }
        }
        let mut driven: Vec<Value> = combos
            .into_iter()
            .map(|(key, mut v)| {
                let best = v["bestLapMs"].as_u64().map(|b| b as u32);
                v["bestLap"] = lap_time(best);
                if let Some(map) = v.as_object_mut() {
                    map.remove("bestLapMs");
                }
                v["lastDriven"] = json!(latest.get(&key));
                v
            })
            .collect();
        driven.sort_by(|a, b| b["lastDriven"].as_str().cmp(&a["lastDriven"].as_str()));
        Ok(json!({ "driven": driven, "onlySharedWithUser": shared_only }))
    }

    async fn list_sessions(&self, args: &Value) -> ToolResult {
        let contains = |hay: &[&str], needle: Option<&str>| {
            needle.is_none_or(|n| {
                let n = n.to_lowercase();
                hay.iter().any(|h| h.to_lowercase().contains(&n))
            })
        };
        let (track, car) = (arg_str(args, "track"), arg_str(args, "car"));
        let limit = arg_limit(args, 20, 100);
        let all = self.sessions().await?;
        let matching: Vec<&SessionSummary> = all
            .iter()
            .filter(|s| contains(&[&s.track_name, &s.track_key], track))
            .filter(|s| contains(&[&s.car_name, &s.car_key], car))
            .collect();
        Ok(json!({
            "total": matching.len(),
            "sessions": matching.iter().take(limit).map(|s| session_json(s)).collect::<Vec<_>>(),
        }))
    }

    async fn get_session(&self, args: &Value) -> ToolResult {
        let id = arg_uuid(args, "session_id")?;
        let Json(detail) =
            crate::data::get_session(State(self.state.clone()), self.auth(), Path(id))
                .await
                .map_err(api_error)?;
        let laps: Vec<Value> = detail
            .laps
            .iter()
            .map(|l| {
                let conditions = l.conditions.as_ref().map(|c| {
                    json!({
                        "weather": c.weather,
                        "airTempC": opt(c.air_temp_c, 1),
                        "trackTempC": opt(c.track_temp_c, 1),
                        "fuelUsed": opt(c.fuel_used, 2),
                        "fuelUnit": c.fuel_unit,
                        "tyreCompound": c.tyre_compound.as_ref().map(|t| t.fl.clone()),
                        "tyreTempC": c.tyre_temp_c.as_ref().map(|w| json!({
                            "fl": round(w.fl, 0), "fr": round(w.fr, 0),
                            "rl": round(w.rl, 0), "rr": round(w.rr, 0)
                        })),
                        "tyreWearPct": c.tyre_wear.as_ref().map(|w| json!({
                            "fl": round(w.fl * 100.0, 0), "fr": round(w.fr * 100.0, 0),
                            "rl": round(w.rl * 100.0, 0), "rr": round(w.rr * 100.0, 0)
                        })),
                    })
                });
                json!({
                    "lapId": l.id.to_string(),
                    "number": l.number,
                    "time": lap_time(l.lap_time_ms),
                    "sectors": l.sector_times_ms.iter()
                        .map(|s| round(*s as f32 / 1000.0, 3)).collect::<Vec<_>>(),
                    "valid": l.valid,
                    "invalidReason": l.invalid_reason,
                    "outLap": l.out_lap.then_some(true),
                    "inLap": l.in_lap.then_some(true),
                    "incomplete": l.incomplete.then_some(true),
                    "flashback": l.flashback.then_some(true),
                    "conditions": conditions,
                })
            })
            .collect();
        Ok(json!({
            "session": session_json(&detail.summary),
            "gameVersion": detail.game_version,
            "laps": laps,
        }))
    }

    async fn best_laps(&self, args: &Value) -> ToolResult {
        let scope = match arg_str(args, "scope").unwrap_or("all") {
            "all" => ReferenceScope::All,
            "mine" => ReferenceScope::Mine,
            "shared" => ReferenceScope::Shared,
            other => return Err(format!("unknown scope {other:?}: use all, mine or shared")),
        };
        let query = ReferenceQuery {
            track: required_str(args, "track_key")?.to_owned(),
            car: required_str(args, "car_key")?.to_owned(),
            scope,
        };
        let Json(laps) =
            crate::sharing::reference_laps(State(self.state.clone()), self.auth(), Query(query))
                .await
                .map_err(api_error)?;
        let limit = arg_limit(args, 10, 50);
        Ok(json!({
            "laps": laps.iter().take(limit).map(|l| json!({
                "lapId": l.lap_id.to_string(),
                "time": lap_time(Some(l.lap_time_ms)),
                "sectors": l.sector_times_ms.iter()
                    .map(|s| round(*s as f32 / 1000.0, 3)).collect::<Vec<_>>(),
                "driver": l.owner.as_ref().map_or_else(|| "you".to_owned(), |o| o.username.clone()),
                "sessionType": session_type(l.session_type),
                "date": l.session_started_at.date().to_string(),
                "track": l.track_name,
                "car": l.car_name,
            })).collect::<Vec<_>>(),
        }))
    }

    async fn compare_laps(&self, args: &Value) -> ToolResult {
        let lap = arg_uuid(args, "lap_id")?;
        let reference = arg_uuid(args, "reference_lap_id")?;
        let data = self.compare(vec![reference, lap], 1.0).await?;
        let lap_info = |i: usize| {
            data.laps.get(i).map(|t| {
                json!({
                    "lapId": t.lap_id.to_string(),
                    "number": t.number,
                    "time": lap_time(t.lap_time_ms),
                    "valid": t.valid,
                    "driver": t.driver.clone().unwrap_or_else(|| "you".into()),
                })
            })
        };
        let gap = match (data.laps.first(), data.laps.get(1)) {
            (Some(r), Some(l)) => l
                .lap_time_ms
                .zip(r.lap_time_ms)
                .map(|(a, b)| (a as f32 - b as f32) / 1000.0),
            _ => None,
        };
        let Some(report) = &data.corners else {
            return Ok(json!({
                "lap": lap_info(1), "reference": lap_info(0), "gapS": opt(gap, 3),
                "corners": [],
                "note": "no corner analysis: the reference lap has no lateral acceleration data",
            }));
        };
        let (ref_stats, lap_stats) = (&report.laps[0].corners, &report.laps[1].corners);
        let mut corners = Vec::new();
        let mut losses: Vec<(f32, u32)> = Vec::new();
        for (i, c) in report.corners.iter().enumerate() {
            let (l, r) = (&lap_stats[i], &ref_stats[i]);
            if let Some(t) = l.time_lost_s {
                losses.push((t, c.number));
            }
            corners.push(json!({
                "corner": c.number,
                "direction": c.direction,
                "apexM": round(c.apex_m, 0),
                "segmentM": [round(c.segment_start_m, 0), round(c.segment_end_m, 0)],
                "minRadiusM": round(c.min_radius_m, 0),
                "timeLostS": opt(l.time_lost_s, 3),
                "entryLostS": opt(l.entry_lost_s, 3),
                "exitLostS": opt(l.exit_lost_s, 3),
                "lap": stats_json(l),
                "reference": stats_json(r),
            }));
        }
        losses.sort_by(|a, b| b.0.total_cmp(&a.0));
        let pick = |items: &mut dyn Iterator<Item = &(f32, u32)>| {
            items
                .take(3)
                .map(|(t, n)| json!({ "corner": n, "timeLostS": round(*t, 3) }))
                .collect::<Vec<_>>()
        };
        let biggest_losses = pick(&mut losses.iter().filter(|(t, _)| *t > 0.01));
        let biggest_gains = pick(&mut losses.iter().rev().filter(|(t, _)| *t < -0.01));
        Ok(json!({
            "lap": lap_info(1),
            "reference": lap_info(0),
            "gapS": opt(gap, 3),
            "biggestLosses": biggest_losses,
            "biggestGains": biggest_gains,
            "corners": corners,
        }))
    }

    async fn lap_telemetry(&self, args: &Value) -> ToolResult {
        let lap = arg_uuid(args, "lap_id")?;
        let reference = match arg_str(args, "reference_lap_id") {
            Some(_) => Some(arg_uuid(args, "reference_lap_id")?),
            None => None,
        };
        let step = args
            .get("step_m")
            .and_then(Value::as_f64)
            .map_or(5.0, |s| s.clamp(1.0, 100.0)) as f32;
        let wanted: Vec<String> = match args.get("channels").and_then(Value::as_array) {
            Some(list) => list
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            None => ["speed", "throttle", "brake", "gear", "steer"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        };
        // the reference first, like the comparison page
        let ids = match reference {
            Some(r) => vec![r, lap],
            None => vec![lap],
        };
        let data = self.compare(ids, step).await?;
        let last = data.dist_m.last().copied().unwrap_or(0.0);
        let from = args.get("from_m").and_then(Value::as_f64).unwrap_or(0.0) as f32;
        let to = args
            .get("to_m")
            .and_then(Value::as_f64)
            .map_or(last, |t| t as f32);
        if to <= from {
            return Err("to_m must be greater than from_m".into());
        }
        let first = data.dist_m.iter().position(|&d| d >= from).unwrap_or(0);
        let end = data
            .dist_m
            .iter()
            .rposition(|&d| d <= to)
            .map_or(0, |i| i + 1);
        if end <= first {
            return Err(format!(
                "no data between {from} and {to} m (the lap is {last:.0} m long)"
            ));
        }
        if end - first > MAX_POINTS {
            return Err(format!(
                "{} points requested, at most {MAX_POINTS}: narrow from_m/to_m or raise step_m",
                end - first
            ));
        }
        let range = first..end;

        let mut units = Map::new();
        let trace = |t: &sy_model::api::LapTrace, with_delta: bool| -> Result<Value, String> {
            let mut channels = Map::new();
            for name in &wanted {
                let Some(values) = t.channels.get(name.as_str()) else {
                    continue;
                };
                let (scale, digits) = match name.as_str() {
                    names::SPEED => (3.6, 1),
                    names::THROTTLE | names::BRAKE | names::CLUTCH => (100.0, 0),
                    names::STEER => (180.0 / std::f32::consts::PI, 1),
                    names::GEAR | names::ABS_ACTIVE | names::TC_ACTIVE => (1.0, 0),
                    names::RPM => (1.0, 0),
                    _ => (1.0, 2),
                };
                channels.insert(
                    name.clone(),
                    Value::Array(
                        values[range.clone()]
                            .iter()
                            .map(|v| round(v * scale, digits))
                            .collect(),
                    ),
                );
            }
            let mut v = json!({
                "lapId": t.lap_id.to_string(),
                "number": t.number,
                "time": lap_time(t.lap_time_ms),
                "driver": t.driver.clone().unwrap_or_else(|| "you".into()),
                "channels": channels,
            });
            if with_delta {
                v["deltaS"] = Value::Array(
                    t.delta_s[range.clone()]
                        .iter()
                        .map(|d| round(*d, 3))
                        .collect(),
                );
            }
            Ok(v)
        };
        for name in &wanted {
            let unit = match name.as_str() {
                names::SPEED => "km/h",
                names::THROTTLE | names::BRAKE | names::CLUTCH => "%",
                names::STEER => "deg",
                _ => data.units.get(name.as_str()).map_or("", String::as_str),
            };
            if data.units.contains_key(name.as_str()) {
                units.insert(name.clone(), json!(unit));
            }
        }
        let missing: Vec<&String> = wanted
            .iter()
            .filter(|n| !data.units.contains_key(n.as_str()))
            .collect();
        let (lap_trace, ref_trace) = match reference {
            Some(_) => (
                trace(&data.laps[1], true)?,
                Some(trace(&data.laps[0], false)?),
            ),
            None => (trace(&data.laps[0], false)?, None),
        };
        Ok(json!({
            "distM": data.dist_m[range.clone()].iter().map(|d| round(*d, 0)).collect::<Vec<_>>(),
            "units": units,
            "lap": lap_trace,
            "reference": ref_trace,
            "deltaNote": reference.map(|_| "deltaS: the lap's time difference to the reference, positive = slower"),
            "unknownChannels": missing,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_for_the_model() {
        assert_eq!(lap_time(Some(125_172)), json!("2:05.172"));
        assert_eq!(lap_time(Some(59_005)), json!("0:59.005"));
        assert_eq!(round(1.23456, 3), json!(1.235));
        assert_eq!(round(4246.4, 0), json!(4246), "whole numbers as integers");
        assert_eq!(round(f32::NAN, 1), Value::Null);
        assert_eq!(
            compact(json!({ "a": null, "b": "", "c": [], "d": { "e": null }, "f": 0 })),
            json!({ "f": 0 })
        );
    }

    #[test]
    fn tool_names_are_unique() {
        let defs = definitions();
        let mut names: Vec<_> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), defs.len());
    }
}
