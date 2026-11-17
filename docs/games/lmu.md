# Le Mans Ultimate – shared memory

Implementation: `crates/sy-source-lmu`. Source: Studio 397's
`SharedMemoryInterface.hpp` and `InternalsPlugin.hpp` from
`<LMU>\Support\SharedMemoryInterface` (the version shipped in March 2026).
They are Studio 397's copyright and may not be redistributed, so they are
not in the repository; put them into `telemetry_examples/LMU/` (ignored by
git) for reference. Our Rust definitions are written from them
(`layout.rs`); the compiler computes every offset, and the sizes are
asserted at compile time.

> **Status:** written from the headers, checked with recordings (Nov
> 2026), on Windows: practice at Monza (BMW M4 LMGT3,
> out-lap and in-lap), a race on Monza's Curva Grande layout (Ford
> Mustang LMGT3: formation lap, rolling start, two track-limits warnings,
> a pit stop), practice at Long Beach (Ferrari 499P: back to the
> garage from the menu twice), qualifying at Road Atlanta in the rain
> (Lexus RCF LMGT3: off into the grass), a race at Sebring's school
> circuit restarted from the menu (BMW) and an online practice at Spa
> (BMW, 30 cars), and on Linux through the Proton bridge a practice at
> Fuji (Cadillac V-Series.R, Bazzite; docs/games/proton.md). They are
> golden tests
> (`crates/sy-capture/tests/lmu_recordings.rs`, skipped without the
> files); synthetic tests (`sy_source_lmu::sim`) cover the same cases.
> Recordings go into `telemetry_examples/LMU/` (raw `.syraw` files,
> *Settings → Advanced → Keep raw recordings*); what is still open is under *To
> verify*.

## Setting up the game

In LMU: *Settings → Gameplay → Enable Plugins* **On**, then restart the
game. No plugin needs to be installed: the setting alone makes the game
publish its shared memory `LMU_Data`. Without it, Stintory sees the game
running (its REST API answers on port 6397) but gets no data, and says so
in the sidebar and under *Settings → Games → Le Mans Ultimate*.

Recording runs on Windows, and on Linux when the game runs in Proton:
there the Proton bridge (`sy-bridge.exe`, started with the game by a Steam
launch option) reads the shared memory inside the game's prefix and passes
it on (docs/games/proton.md). Instead of the bridge, Stintory can also read
`/dev/shm/LMU_Data` when another shared-memory bridge
([simshmbridge](https://github.com/Spacefreak18/simshmbridge),
[DataLink](https://github.com/LukasLichten/Datalink)) puts the game's
memory there (docs/games/proton.md, *Other shared-memory bridges*). Other telemetry apps keep working: the
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
  the header). Without them (older builds) we poll every 10 ms. The
  telemetry comes at 100 Hz (`mDeltaTime` 0.01 s; 50 Hz in the garage), the scoring at 5 Hz;
  a few samples are missed (up to 0.1 s in the recordings).
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

- Only the player's car (docs/ARCHITECTURE.md §5.2). Spooled records: the player's
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
  wear (0 new; the game's `mWear` is what is left, 1 when new, although
  the header calls it wear), brake temperature (°C), ride height,
  suspension deflection and surface (0 dry, 1 wet, 2 grass, 3 dirt, 4
  gravel, 5 rumble strip, 6 special).
- Steering: `mPhysicalSteeringWheelRange` was 0 for all three cars
  (`mVisualSteeringWheelRange` 719° for all). The setup's
  `VM_STEER_LOCK` has the car's range ("516 deg(19.7 )" BMW, "435 (17.7)
  deg" Ford, "400 (16) deg" Ferrari: the wheel, then the front wheels);
  `steer` uses it when the game has none.
- Virtual energy and the battery are 0–1 (the recordings start at 1 out
  of the garage); regen is in kW in the game (up to 194 kW for the
  Ferrari), W in Stintory.
- Rain: `mRaining` (0–1) and the racing line's wetness
  (`mAvgPathWetness`, 0–1; the minimum stays 0, the maximum is the
  average or a step ahead) come from the scoring, as `lmu.rain` and
  `lmu.track_wetness` in every sample, and in the session's weather
  ("rain 30 %, track wet 40 %" at Road Atlanta). `mDarkCloud` stayed 0 in
  the rain. On a wet track the tyres' surface is 1 (wet) instead of 0.
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
  types are short segments at the pit boxes. The shared memory's `mPos`
  is in the same coordinates (Monza: median 1.8 m from the map's line).
  Not used yet.

**Not every `GET` is harmless**: `/rest/options/resetVRView` resets the VR
view, `/rest/multiplayer/join` joins a server, `/rest/watch/play/{id}`
starts a replay, `/rest/profile/getAuthSessionTicket` returns a Steam
authentication ticket. Stintory calls only the endpoints listed in
`rest.rs` (today only the setup), never anything else, and never with
another method.

## Sessions, laps, validity

- A session is one session number (`mSession`: 0 test day, 1–4 practice,
  5–8 qualifying, 9 warm-up, 10–13 race) on one track with one car. The
  elapsed time going back starts a new session: "Restart race" in the
  menu puts the car into the garage (like going back to the garage),
  then the session starts again at 0. The game's session started at the
  scoring's receive time minus the elapsed time; offline that is the
  session's start in Stintory. Online the server's session may have run
  for long (42 minutes at Spa) before the player joined, so there the
  session starts at the player's first scoring record.
  Keys: session `lmu:<driver hash>:<game's session start / 10
  s>:<session>:<vehicle hash>` (reconnecting to an online session keeps
  it), track `lmu:<track name>:<length in m>`, car
  `lmu:<model>:<class>` (e.g. `lmu:porsche_963:hypercar`; LMP2 in WEC and
  ELMS trim are different cars).
- The game's track name is the venue for a venue's main layout
  ("Autodromo Nazionale Monza", 5780.6 m), but other layouts can have
  their own ("Monza Curva Grande Circuit", 5745.1 m; the REST track list
  calls both 5.793 km). Whether every layout has its own name is not
  known (Bahrain has four, Paul Ricard five), so the layout's length from
  the scoring (`mLapDist`) is part of the track key and the name
  ("Autodromo Nazionale Monza (5.78 km)").
- **Lap distance** is only in the scoring, five times a second. A frame's
  distance is the last scoring distance plus speed × time since; the error
  found at the next scoring update is corrected linearly over 0.2 s, so
  the distance stays continuous. At the line (the telemetry's lap number
  goes up) it restarts from the lap's start time `mLapStartET`. With
  synthetic data (speed ±40 % along the lap) it stays within 1.5 m.
- Laps: the telemetry's `mLapNumber` and the scoring's `mTotalLaps` are
  the same number (laps completed = the lap being driven, from 0). The
  out-lap from the garage is lap 0. Crossing the line in the pit lane
  counts a lap.
- Lap times: when the player's `mTotalLaps` goes up, `mLastLapTime` and
  `mLastSector1`/`mLastSector2` (cumulative) are the official times of the
  lap that ended. A time ≤ 0 (−1) means the game didn't time the lap:
  out-laps, in-laps outside races (in a race the in-lap is timed), and a
  lap with a track-limits warning (`mTrackLimitsSteps` went up; the game
  also stops timing the sectors then), or a lap the game stopped
  counting off the track without a warning (Road Atlanta: sliding into
  the grass). Such laps are invalid: "track limits", "off track", or
  "not timed by the game" (out- and in-laps).
- `mCountLapFlag` (header: 0 lap and time not counted, 1 lap only, 2 both)
  goes to 1 when the car leaves the track and back to 2 when the game
  forgives it (cutting a corner without gaining anything gives no
  warning and the lap is timed); it stays 1 after a warning or a real
  excursion. It is 1 on out-laps and from the pit entry. A lap whose
  flag went from 2 to 1 on track and didn't come back is "off track".
- `mLapInvalidated` was never set, neither with warnings nor for cuts;
  when it is, the lap is invalid ("track limits" or "invalidated by the
  game").
- Out-lap: the lap starts in the pit lane (sign bit of `mCurrentSector`,
  or `mInPits`); in-lap: it ends there. `mPitState` is 5 in the garage
  (the header lists 0–4); 1 request, 2 entering, 4 in the pit box /
  leaving.
- **Back to the garage** from the menu: the car is put into the garage
  (its position jumps), the lap number and `mTotalLaps` go up (no time,
  `mLapStartET` a moment in the future), and 0.1 s later the scoring says
  `mInRealtime` false. Stintory ends the lap where the car was, as
  incomplete ("back to the garage"); from the pit box (at Long Beach it
  is before the line) that is an incomplete in-lap. Out of the garage
  again, the next lap number is an out-lap. Leaving the car otherwise
  (to the monitor) ends the lap as incomplete ("left the car").
- **Races:** before the start (`mGamePhase` 0–4: formation lap,
  countdown) nothing is recorded. The game has only rolling starts:
  "Rolling" (a formation lap) and "Fast" (closer to the line, without a
  whole lap); no standing starts. At the green flag `mLapStartET` is set
  and the lap number stays 0: the first lap starts before the line (62 m
  at Monza, 150–185 m at Sebring) and its distance counts up from there
  to 0 at the line. It ends at the line a whole lap later, and the game
  times it from the green flag. After the flag (`mGamePhase` 8) the
  cool-down lap is recorded until the player leaves the car.
- A "lap" shorter than a second (100 samples) isn't kept: the line
  crossed just before going back to the garage or restarting.
- Pause: the elapsed time stands still and nothing new arrives. While the
  player is on track the recording waits up to an hour; otherwise 10 s of
  silence end it.

## Axes and signs

- The world is left-handed with +y up. `pos_x` = x, `pos_y` = z gives an
  unmirrored map: the game's own track map of Monza (REST, same world
  coordinates) runs clockwise that way, as the real track does.
- Checked with the recordings (steering right turns the path clockwise on
  that map; all three tracks are driven clockwise, as the real ones):
  car coordinates +x left, +y up, +z back. `g_lat` = −x (positive in
  right-hand turns), `g_long` = −z (positive accelerating), `yaw` from the
  orientation matrix (the car's forward direction is local −z; it agrees
  with the direction of travel within 0.05 rad), yaw rate = `mLocalRot.y`
  (+ = to the right). `mUnfilteredSteering` is + to the right.

## To verify with recordings

- The Hold/Data events and the lock: the Windows recordings have every
  100 Hz sample once, but a spool file doesn't show whether the reader
  waited on the events or polled.
- Linux through the Proton bridge (docs/games/proton.md).
- Full-course yellow (`mGamePhase` 6; not reproduced offline).
- Whether `mLapInvalidated` is ever set.
- Rain (`mRaining`, the surface types), the tyre wear over a long stint.
