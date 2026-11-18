# iRacing – verified behavior

Implementation: `crates/sy-source-iracing`. Sources: the official iRacing SDK 1.20 (`irsdk_defines.h`, `irsdk_utils.cpp`, `irsdk_diskclient.cpp`) plus analysis of our own `.ibt` recordings (Snetterton 300 / GR86, Summit Point / MX-5, Oschersleben GP / Ray FF1600, Winton National / MX-5 incl. a live recording on Windows, Nov 2026, sim build 2026.11.02.01). Everything below was checked against those recordings.

## Format

- Header 112 bytes. SDK 1.20 added `curBufTickCount`, `curBuf` and per-buffer `tickCountBegin` for torn-read detection.
- `.ibt`: header + 32-byte disk sub header. Data rows follow back to back from `varBuf[0].bufOffset`. `numBuf` is 1. `sessionRecordCount` matched the file size in all samples; we still trust the file size.
- Variables are packed **without alignment** (e.g. `Speed` at offset 314), so all reads are unaligned.
- 60 rows/s. `.ibt` files have ~280 variables.
- **`.ibt` contains no `CarIdx*` arrays.** The only array is `SteeringWheelTorque_ST[6]`. The live data does have `CarIdx*` arrays; we zero other cars' entries before spooling.
- **`.ibt` contains GPS**: `Lat`/`Lon` (double, degrees), `Alt` (m). We project to local meters around `WeekendInfo.TrackLatitude/TrackLongitude`. Those fields have a bogus unit "m" but contain degrees. That point is the **start/finish line** (the car crosses it at < 3 m from the origin).
- **Live telemetry has no GPS** (a live recording had no track map; the same session's `.ibt` had one). Instead we integrate `VelocityX` (forward) and `VelocityY` (left), both m/s in the car frame, rotated by `YawNorth` (heading from north, clockwise): `east += (vx·sin h − vy·cos h)·dt`, `north += (vx·cos h + vy·sin h)·dt`.
  - Checked by replaying the Winton `.ibt` with `Lat`/`Lon` hidden: < 2 m drift over 7 minutes without resets.
  - The position is set to (0, 0) at every line crossing **on track**. That bounds the drift (< 4 m from GPS in every lap) and puts it in the same frame as the GPS position.
  - Before the first crossing on track (out-lap from the pits, joined mid-lap, after a tow) the position is unknown and left empty. The line crossing in the pit lane is ~25 m off the racing line, so it is no anchor.
- Session info YAML is ISO-8859-1. It contains **all drivers** (names, ids, iRating, license) and results. We keep only a whitelist (`session_info.rs`).

## Lap behavior

- `Lap` = lap being driven, `LapCompleted` = laps done. Both increment at the same row when crossing the line.
- `LapLastLapTime` (official time of lap `LapCompleted`) updates **1–2.2 s after** the crossing.
  - It becomes `-1` when iRacing invalidated the lap (seen after incidents).
  - It **often stays unchanged** (e.g. remains `-1` over several clean laps in a race, or keeps the previous value). An unchanged value carries no information.
  - We use only changes: > 0 → official time; → -1 → lap invalid. Otherwise the lap keeps its measured time.
- The official time of a lap that **includes a pit stop** is reported (146.105 s at Oschersleben).
- **The lap counter can go back down**:
  - Seen in qualifying when leaving the pits: the pit box is just before the line, so `Lap` 0 → 1 → 0.
  - Seen once mid-session: 3 → 4 → 3 five seconds after crossing the line; no official time for lap 3.
  - We mark the taken-back lap "not counted by the game".
- Crossing the line in the pit lane: `LapDist` is slightly negative (-0.337) and `OnPitRoad` is true. The lap ending there is an in-lap, the next one an out-lap.
- **Pit box before the line** (Winton): "lap 0" is the 7 s from the box to the line, all in the pit lane. After a reset to the pits, the seconds in the box are another such fragment. A "lap" that never leaves the pit lane is dropped.
- Race start from the grid: the first row is just before the line (`LapDist` 3606 of 3610 m), lap 0 → incomplete.
- **One session can span several `.ibt` files.** The next file starts at the session time where the previous ended (71.08 → 71.10 s), mid-lap. `sessionStartDate − sessionStartTime` is the same for both, so it is used as the session's wall-clock start. The importer reads files of a session in order as one stream.
- **Session key**: `iracing:<SubSessionID>:<SessionNum>:<player UserID>`; offline (`SubSessionID` 0): `iracing:offline:<TrackID>:<CarID>:<start minute>:<SessionNum>:<player UserID>`. The driver is part of the key because everyone in an online session has the same subsession id, and session/lap ids derived from the key must stay unique when laps are exchanged.
- **The same session recorded live and imported from its `.ibt`** must not become two sessions. Offline sessions have no session id; their key contains the start minute, which the two sources can see differently (live: `now − SessionTime` when connecting). The store therefore treats a session with the same track, car and type that started ≤ 2 min apart as the same (the new id becomes an alias). Laps match on number and start session time (±0.5 s); the stored lap gets the file's telemetry if it has more channels or samples (GPS).
- The **first row of each file is stale**: `PlayerTrackSurface = -1`, `LapDist = 0`, `LapCompleted = 0`. Rows with surface -1 or `IsOnTrack = false` are skipped.

## Validity (our rules)

iRacing has no live "lap invalid" flag. A lap is invalid for us when, during the lap:
- `PlayerCarMyIncidentCount` increases ("incident (+Nx)"),
- `PlayerTrackSurface` becomes `OffTrack` ("off track"),
- iRacing changes `LapLastLapTime` to -1 ("invalidated by iRacing"),
- the lap counter goes back ("not counted by the game"), or
- the car is on track (not in the pit lane) while `SessionFlags` has the **meatball** (`irsdk_repair`, 0x00100000), black (0x00010000) or disqualify (0x00020000) bit set.

The meatball rule comes from the Oschersleben race:
- After the incidents in lap 2 the repair flag was set (251 s) until the car was in the pit box (622 s, lap 6).
- iRacing didn't count laps 2–5. `LapLastLapTime` just stayed at -1, so without the flag rule laps 3 and 4 looked valid.
- The out-lap with the pit stop (lap 6) was counted (146.105 s), although the flag was still set while in the pit lane. That's why time in the pit lane is excluded.

In races iRacing reports times for laps with an off-track moment. We still mark those laps invalid, because they aren't clean reference laps.

## Sign conventions

| iRacing variable | iRacing sign | Our channel |
|---|---|---|
| `SteeringWheelAngle` (rad) | positive = **left** (checked against GPS heading: 1399 vs 18 samples) | `steer` = −value |
| `LatAccel` | same sign as steering | `g_lat` = −value |
| `YawRate` | same sign as steering | `yaw_rate` = −value |
| `Clutch` | 0 = disengaged … 1 = engaged | `clutch` (pedal) = 1 − value |
| `dcBrakeBias` | percent front (e.g. 56.0) | `brake_bias` = value / 100 |
