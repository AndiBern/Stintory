# Le Mans Ultimate – shared memory

Implementation: `crates/sy-source-lmu`. Source: Studio 397's
`SharedMemoryInterface.hpp` and `InternalsPlugin.hpp` from
`<LMU>\Support\SharedMemoryInterface` (the version shipped in March 2026).
They are Studio 397's copyright and may not be redistributed, so they are
not in the repository; put them into `telemetry_examples/LMU/` (ignored by
git) for reference. Our Rust definitions are written from them
(`layout.rs`); the compiler computes every offset, and the sizes are
asserted at compile time.

> **Status:** written from the headers and tested with synthetic data
> (`sy_source_lmu::sim`). Not yet checked with the real game; see *To
> verify*. Recordings go into `telemetry_examples/LMU/` (raw `.syraw`
> files, *Settings → Keep raw recordings*).

## Setting up the game

In LMU: *Settings → Gameplay → Enable Plugins* **On**, then restart the
game. No plugin needs to be installed: the setting alone makes the game
publish its shared memory `LMU_Data`. Without it, Stintory sees the game
running (its REST API answers on port 6397) but gets no data, and says so
in the sidebar and under *Settings → Le Mans Ultimate*.

Recording runs on Windows, and on Linux when the game runs in Proton:
there the Proton bridge (`sy-bridge.exe`, started with the game by a Steam
launch option) reads the shared memory inside the game's prefix and passes
it on (docs/games/proton.md). Other telemetry apps keep working: the
shared memory has any number of readers.

## Reading the shared memory

- `LMU_Data` is one `SharedMemoryObjectOut` (324,824 bytes): `generic`
  (event queue, game version, app state), `paths`, `scoring`
  (`ScoringInfoV01` + `VehicleScoringInfoV01[104]` + results stream) and
  `telemetry` (`playerVehicleIdx`, `playerHasVehicle`, `TelemInfoV01[104]`).
- The structs from `InternalsPlugin.hpp` are packed to 4 bytes; the
  `SharedMemory*` wrappers use the default packing (8), so there are 4
  padding bytes before `scoringStreamSize`. `TelemInfoV01` (1888 bytes),
  `TelemWheelV01` (260), `VehicleScoringInfoV01` (584) and
  `ScoringInfoV01` (548) have their rFactor 2 sizes: LMU's additions
  (virtual energy, TC/ABS settings, motor map, …) went into the reserved
  space.
- New frames are signalled by two events, `LMU_Data_HoldEvent` and then
  `LMU_Data_DataEvent` (waiting in that order is the race-free way, says
  the header). Without them (older builds) we poll every 10 ms.
- The game writes under a lock: the mapping `LMU_SharedMemoryLockData`
  (two `LONG`s, `waiters` and `busy`) and the event
  `LMU_SharedMemoryLockEvent`. We take it the way the header's
  `SharedMemoryLock` does (spin on `busy`, then wait as a registered
  waiter), but give up after 20 ms so the game never waits on us for long.
  Without the lock a copy counts only if the elapsed time didn't change
  while copying.
- `generic.events` is a queue of event ids (`SME_UPDATE_TELEMETRY`, …), not
  a set of counters. We don't rely on it: new data is new when
  `mElapsedTime` (telemetry) or the scoring changed.
- Under the lock we copy only the session scoring, the player's scoring
  entry (the one with `mIsPlayer`, found by reading just that byte of each
  entry) and the player's telemetry. `telemInfo[playerVehicleIdx]` must
  have the slot id of the player's scoring entry; otherwise the slots are
  searched by id.

## What is recorded

- Only the player's car (docs/PLAN.md §5.2). Spooled records: the player's
  `TelemInfoV01`, `ScoringInfoV01` and the player's `VehicleScoringInfoV01`
  (with the results-stream and vehicle pointers, the server's address,
  port and name, the player name and the Steam id zeroed), and the car
  setup from the REST API. Other cars' scoring and telemetry are never
  copied, let alone written.
- One sample per telemetry update while driving (`mInRealtime`; nothing in
  the garage or at the monitor). Canonical channels: speed (‖`mLocalVel`‖),
  throttle, brake, clutch (inputs before aids), `steer_norm` and `steer`
  (input × physical wheel range / 2), gear, rpm, position, g-forces, yaw,
  yaw rate, fuel (l), brake bias (1 − `mRearBrakeBias`), ABS and TC active.
- Extras `lmu.*`: virtual energy, battery, regen (W), electric motor
  torque/rpm/state, TC level/slip/cut, ABS level, motor map, migration,
  anti-roll bars, lift-and-coast, track-limits steps, lap invalidated,
  speed limiter, engine torque, water/oil temperature, ride heights and
  downforce front/rear; per wheel (`_fl`, `_fr`, `_rl`, `_rr`) tyre tread
  temperature (average of the three zones), carcass temperature, pressure,
  wear, brake temperature, ride height, suspension deflection and surface
  (0 dry, 1 wet, 2 grass, 3 dirt, 4 gravel, 5 rumble strip, 6 special).
- The lap view shows virtual energy / battery, ABS and TC active, the TC
  and ABS levels and the motor map when a lap has them.
- Session info (player only): session number and name, track length, lap
  and time limit, online or not, field size, fixed setup, track grip,
  track-limits rules, vehicle, model, class, championship (BoP year),
  steering range, gears, fuel capacity, tyre compounds.
- Car setup: `GET /rest/garage/getPlayerGarageData` every 5 s, stored when
  it changed (exported only when chosen). The response (~40 KB) has one
  object per setting with the readable `stringValue`, an index `value`,
  limits and UI state; Stintory keeps `{"VM_BRAKE_BALANCE": "51.2:48.8",
  "WM_PRESSURE-W_FL": "136 kPa", …}` (about 120 settings, ~4 KB) and drops
  settings the car doesn't have. The raw response stays in the spool.

## The REST API

The game's OpenAPI file (`/swagger-schema.json`, 183 paths) names the
endpoints but documents no responses. What we learned from responses
saved in the pits (Monza, Nov 2026):

- `GET /rest/garage/getPlayerGarageData`: the setup, see above. No
  personal data.
- `GET /rest/watch/sessionInfo`: the same fields as `ScoringInfoV01`
  (`trackName` "Autodromo Nazionale Monza", `lapDistance` 5780.6, `session`
  "PRACTICE1", `maximumLaps` 4294967295 = no limit, player and server
  name). Nothing the shared memory doesn't have.
- `GET /rest/race/track`: every track the game has (39), not the current
  one: event name ("6 Hours of Monza", "Monza Curva Grande Circuit"),
  venue, scene (`MONZAWEC`, `MONZAWEC_GRANDE`) and a rounded length.
- `GET /rest/watch/trackmap`: the current track as points in world
  coordinates (x, y, z with y up): type 0 is the racing surface as a
  closed loop (5 m apart, 5761 m at Monza), type 1 the pit lane, the other
  types are short segments at the pit boxes. A possible source for track
  outlines later (docs/PLAN.md §6.3); not used yet.

**Not every `GET` is harmless**: `/rest/options/resetVRView` resets the VR
view, `/rest/multiplayer/join` joins a server, `/rest/watch/play/{id}`
starts a replay, `/rest/profile/getAuthSessionTicket` returns a Steam
authentication ticket. Stintory calls only the endpoints listed in
`rest.rs` (today only the setup), never anything else, and never with
another method.

## Sessions, laps, validity

- A session is one session number (`mSession`: 0 test day, 1–4 practice,
  5–8 qualifying, 9 warm-up, 10–13 race) on one track with one car. The
  elapsed time going back (restart) starts a new session. Its wall-clock
  start is the scoring's receive time minus the elapsed time.
  Keys: session `lmu:<driver hash>:<start / 10 s>:<session>:<vehicle
  hash>`, track `lmu:<track name>:<length in m>`, car
  `lmu:<model>:<class>` (e.g. `lmu:porsche_963:hypercar`; LMP2 in WEC and
  ELMS trim are different cars).
- The game's track name is the **venue**, the same for all its layouts
  (Bahrain has four, from 3.5 to 6.3 km; Paul Ricard five). The layout's
  length from the scoring (`mLapDist`, 5780.6 m at Monza) tells them
  apart, so it is part of the track key and the name ("Autodromo
  Nazionale Monza (5.78 km)"). The game has no field with the layout's
  name; its track list (REST) has only rounded lengths, some equal for
  different layouts.
- **Lap distance** is only in the scoring, five times a second. A frame's
  distance is the last scoring distance plus speed × time since; the error
  found at the next scoring update is corrected linearly over 0.2 s, so
  the distance stays continuous. At the line (the telemetry's lap number
  goes up) it restarts from the lap's start time `mLapStartET`. With
  synthetic data (speed ±40 % along the lap) it stays within 1.5 m.
- Lap times: when the player's `mTotalLaps` goes up, `mLastLapTime` and
  `mLastSector1`/`mLastSector2` (cumulative) are the official times of the
  lap that ended. A time ≤ 0 means the game didn't time the lap: invalid,
  "not timed by the game".
- A lap is invalid when the game sets `mLapInvalidated` during it. When the
  track-limits steps (`mTrackLimitsSteps`) went up in that lap, the reason
  is "track limits", otherwise "invalidated by the game".
- Out-lap: the lap starts in the pit lane (sign bit of `mCurrentSector`,
  or `mInPits`); in-lap: it ends there. Going back to the garage or the
  monitor mid-lap ends the lap as incomplete ("left the car").
- Pause: the elapsed time stands still and nothing new arrives. While the
  player is on track the recording waits up to an hour; otherwise 10 s of
  silence end it.

## Axes and signs

- The world is left-handed with +y up. `pos_x` = x, `pos_y` = z gives an
  unmirrored map: the game's own track map of Monza (REST, same world
  coordinates) runs clockwise that way, as the real track does.
- From the header, to verify with a recording:
  car coordinates +x left, +y up, +z back. `g_lat` = −x (positive in
  right-hand turns), `g_long` = −z (positive accelerating), `yaw` from the
  orientation matrix (the car's forward direction is local −z), yaw rate
  = `mLocalRot.y` (+ = to the right).

## To verify with recordings

- The struct layout against the live mapping (the mapping must be at
  least 324,824 bytes; otherwise the settings say the layout is unknown).
- That the Hold/Data events and the lock exist and behave as the header
  says; the telemetry rate.
- Lap numbering: `mLapNumber` vs `mTotalLaps`, the out-lap from the
  garage, the race start from the grid, the lap after the flag.
- Whether out-laps have a time (`mLastLapTime`), and `mCountLapFlag`.
- `mVirtualEnergy`'s unit (assumed 0–1; the setup shows the starting
  energy as "100%"), `mPhysicalSteeringWheelRange` in degrees (the setup's
  steering lock was "516 deg" at Monza), `mBrakeTemp` in °C (the header
  says so).
- That `mPos` is in the same coordinates as the REST track map; the signs
  of g-forces, yaw and yaw rate (see above).
- That the scoring's track length is the same in every session on a
  layout, and differs between Monza's two layouts.
