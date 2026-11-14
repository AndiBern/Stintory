# Stintory – Research Findings

Research done 2026-11-10, before the implementation started; trimmed 2026-11-14. This file is kept for three things:

1. The record of the public, non-code facts and sources the parsers rely on ([ARCHITECTURE.md §12.2](ARCHITECTURE.md#122-parsers-from-scratch-clean-room)).
2. An overview of what each game's interface offers, including data Stintory doesn't use yet. It's a starting point for new features.
3. The competitor overview.

What Stintory actually does, checked with the real games, is in [games/](games/). Where the two disagree, games/ is right. Facts still marked **(verify)** come from community sources only and haven't been confirmed.

---

## 1. Telemetry interfaces per game

### Summary

| | iRacing | F1 25 (+ 2026 Season Pack) | Le Mans Ultimate |
|---|---|---|---|
| Mechanism | Windows shared memory (`Local\IRSDKMemMapFileName`) + data-ready event; `.ibt` files on disk | UDP packets to a configurable IP/port (default `20777`) | Built-in Windows shared memory `LMU_Data` + local REST API on `127.0.0.1:6397` |
| Official docs | iRacing SDK (`irsdk_defines.h`, sample code) | EA "F1 25 UDP Specification" (C++ header, v9.0 for 2026 Season Pack) | `SharedMemoryInterface.hpp` shipped in the game folder `Support\SharedMemoryInterface` |
| Rate | 60 Hz live; 60 Hz in `.ibt` (some `_ST` channels at 360 Hz) | Configurable: 10/20/30/40/50/60 Hz | Telemetry 100 Hz (50 Hz in the garage), scoring 5 Hz (checked, games/lmu.md) |
| Setup needed by user | None for live; `.ibt` needs telemetry recording armed (Alt+L or the in-sim option) | In-game: UDP Telemetry On, IP/port, send rate, UDP format 2025/2026 | In-game: Settings → Gameplay → **Enable Plugins = ON**, then restart. No DLL needed anymore |
| Other cars | Limited `CarIdx*` arrays (lap %, position, gear, RPM, steer, lap times) | Full motion/telemetry for all 22/24 cars (subject to other players' "Your Telemetry" setting online) | Full telemetry + scoring for up to 104 vehicles |
| Runs on consoles | – | Yes (PS5/Xbox can send UDP to the PC client) | – |
| Linux/Proton | iRacing doesn't support Proton; `.ibt` import works everywhere | UDP works through Proton (network) | Through Stintory's `sy-bridge.exe` (games/proton.md) |

---

### 1.1 iRacing

**Live telemetry (shared memory)**
- Memory-mapped file `Local\IRSDKMemMapFileName`, plus an event `Local\IRSDKDataValidEvent` that is signalled for every new sample (60 Hz). You can block on the event instead of polling.
- Layout: a header (`irsdk_header`: version, status, tickRate, session info offset/length/update counter, number of variables, var-header offset, 4 rotating data buffers with `tickCount`) → variable header table (name, type, offset, count, unit, description) → data buffers. The variable set is **self-describing**: the client discovers names, types and units at runtime. No hardcoded struct.
- Read safely: take the buffer with the highest `tickCount`, copy it, then check that `tickCount` didn't change during the copy.
- **Session info** is a YAML string (WeekendInfo with TrackID/TrackName/TrackConfigName, DriverInfo with CarID/CarScreenName/UserID/CustID, SessionInfo with results, CarSetup). It has its own update counter and changes rarely.
- Broadcast messages (`RegisterWindowMessage("IRSDK_BROADCASTMSG")`) can start/stop/restart **disk telemetry recording** (`irsdk_BroadcastTelemCommand`). The client can use this to arm `.ibt` recording automatically.

**Disk telemetry (`.ibt`)**
- Same header/var-table format, followed by all samples. Written from entering to leaving the car into `Documents/iRacing/telemetry/<car>/`.
- Contains channels that are not available live. GPS `Lat`/`Lon`/`Alt` are disk-only (confirmed, games/iracing.md). The reverse is also true: some live channels are missing from `.ibt`.
- `.ibt` import is pure file parsing, so it also **works on Linux**. Users can import files they copied over, or files from other people.

**Most relevant channels** (≈200 in total)

| Category | Channels (unit) |
|---|---|
| Inputs | `Throttle`, `Brake`, `Clutch` (0–1), `*Raw` variants, `SteeringWheelAngle` (rad), `SteeringWheelAngleMax`, `SteeringWheelTorque` (N·m) |
| Engine/drive | `Speed` (m/s), `RPM`, `Gear` (-1/0/1..n), `ShiftIndicatorPct` |
| Position on track | `LapDist` (m), `LapDistPct` (0–1), `Lap`, `LapCompleted`, `OnPitRoad`, `PlayerTrackSurface` |
| Timing | `LapCurrentLapTime`, `LapLastLapTime`, `LapBestLapTime` (s), `LapDeltaToBestLap`/`OptimalLap`/`SessionBestLap` (+ `_OK` flags), `SessionTime` (double) |
| Dynamics | `LatAccel`, `LongAccel`, `VertAccel` (m/s²), `Yaw`/`Pitch`/`Roll` (rad), `YawRate` etc. (rad/s), `VelocityX/Y/Z` (m/s, car frame), `YawNorth` |
| 360 Hz (`_ST`, float[6] per 60 Hz tick) | shock deflection/velocity per corner, accelerations, rotation rates, velocities, `SteeringWheelTorque_ST` |
| Tyres | carcass temps `LFtempCL/CM/CR`…, wear `LFwearL/M/R`…, `LFcoldPressure`… (some only update in the pits) |
| Car controls | `dcBrakeBias`, `dcABS`, `dcTractionControl` … (`dc*` = in-car adjustments, car dependent), `BrakeABSactive` |
| Fuel | `FuelLevel` (l), `FuelLevelPct`, `FuelUsePerHour` (kg/h) |
| Session/flags | `SessionFlags` (bitfield), `SessionState`, `SessionNum`, `PlayerCarMyIncidentCount`, `PlayerCarPosition` |
| Other cars | `CarIdxLapDistPct`, `CarIdxLap`, `CarIdxPosition`, `CarIdxLastLapTime`, `CarIdxBestLapTime`, `CarIdxGear`, `CarIdxRPM`, `CarIdxSteer`, `CarIdxTrackSurface`, `CarIdxOnPitRoad` (64 entries) |

**Points to note for implementation**
- iRacing has **no "lap invalid" flag**. Validity has to be derived from incident-count changes, off-track surface (`PlayerTrackSurface`), and pit-road usage.
- Track map from live data: there is no world position live. Stintory integrates velocity + heading (dead reckoning, games/iracing.md) and uses `Lat/Lon` from `.ibt`. Track outlines could be cached per track after the first lap.
- Track/car identity: `TrackID` + `TrackConfigName`, `CarID` / `CarPath` from the session YAML. These are stable across users, which is good for cross-user comparison.
- Implementation source: the **official iRacing SDK** (`irsdk_defines.h` + docs). Existing community libraries (`pyirsdk`, `iracing`/`pitwall`/`kerb` crates, `teamjorge/ibt`, SVappsLAB SDK) are listed only to show the landscape. Per ARCHITECTURE §12.2 we neither use nor port them.

---

### 1.2 F1 25 / "F1 26"

**Important:** there is **no standalone F1 26 game**. In Nov 2025 EA announced it would skip 2026 and sell the **F1 25: 2026 Season Pack** (released 3 June 2026) instead. The next full game is planned for 2027. "F1 26" support means supporting the **2026 UDP format** of F1 25.

**Mechanism:** the game sends UDP packets (unicast or broadcast) to an IP/port. Send rate 10–60 Hz. The player picks a "UDP Format" (2026 by default on new installs, or older formats for legacy tools). Packets are little-endian and packed. Every packet starts with a 29-byte header:

```
uint16 packetFormat   // 2025 or 2026 → selects the parser
uint8  gameYear       // NOTE: stays "25" even in 2026 mode – don't use for detection
uint8  gameMajorVersion, gameMinorVersion, packetVersion, packetId
uint64 sessionUID     // groups packets into a session
float  sessionTime
uint32 frameIdentifier, overallFrameIdentifier (doesn't reset on flashback)
uint8  playerCarIndex, secondaryPlayerCarIndex
```

**Packet types (2026 format; sizes from the official v9.0 header)**

| ID | Packet | Size | Content |
|---|---|---|---|
| 0 | Motion | 1325 B | World position/velocity, forward/right vectors, G-forces lat/long/vert, yaw/pitch/roll **for all cars** |
| 1 | Session | 926 B | Weather + forecast, track/air temp, trackId, track length, session type, formula, assists, marshal zones, sector 2/3 start distance, **active aero zones, DRS zones**, rule settings |
| 2 | Lap Data | 1399 B | Last/current lap time (ms), sector 1/2 times, gaps, **lapDistance**, totalDistance, position, lap number, pit status, sector, **currentLapInvalid**, penalties, warnings, driverStatus (out/in/flying lap) |
| 3 | Event | 45 B | Session start/end, fastest lap, penalties, speed trap, start lights, **flashback** (frame id + session time), buttons, overtake, safety car, collision |
| 4 | Participants | 1470 B | Names, team, race number, AI/human, platform, **yourTelemetry** (restricted/public) |
| 5 | Car Setups | 1233 B | Wings, diff, camber/toe, suspension, ARBs, ride height, brake pressure/bias, tyre pressures, fuel load |
| 6 | Car Telemetry | 1448 B | Speed (km/h), throttle, steer (-1..1), brake, clutch, gear, RPM, DRS, brake temps, tyre surface/inner temps, engine temp, tyre pressures, surface type |
| 7 | Car Status | 1445 B | TC, ABS, fuel mix, brake bias, fuel in tank/capacity/remaining laps, max RPM, tyre compound (actual/visual), tyre age, ERS store/deploy mode/harvested/deployed, ICE & MGU-K power |
| 8 | Final Classification | 1134 B | Results, best lap, stints |
| 9 | Lobby Info | 1062 B | Multiplayer lobby |
| 10 | Car Damage | 1133 B | Tyre wear/damage/blisters, wing/floor/diffuser/sidepod damage, engine component wear |
| 11 | Session History | 1460 B | Per car (cycled): up to 100 laps with lap + sector times and validity bits, tyre stints |
| 12 | Tyre Sets | 231 B | Per car (cycled): 20 tyre sets with wear, life, delta |
| 13 | Motion Ex | 273 B | **Player only**: suspension pos/vel/acc, wheel speed, slip ratio/angle, lat/long/vertical wheel forces, local velocity, angular velocity/acceleration, front wheel angle, aero heights, roll angles, chassis yaw/pitch, camber |
| 14 | Time Trial | 104 B | Session best / personal best / rival lap + sectors + assists |
| 15 | Lap Positions | 1231 B | Position of every car at the start of each lap |
| 16 | **Car Telemetry 2 (new in 2026)** | 269 B | Active aero mode/availability/activation distance, overtake mode available/active/distance, 2026-regulations flag, driving wrong way |

**Points to note for implementation**
- **Formats differ:** the 2026 format has 24 cars (11 teams). The 2025 format has 22, and has no packet 16. The parser must branch on `packetFormat`. Using the 2025 format with 2026 cars drops boost/ERS data and labels cars "F1 Generic" (per F1Laps).
- **Flashbacks:** an Event `FLBK` with the frame/session time it rewinds to. The recorder must truncate the current lap's samples back to that time. Usually a lap with a flashback is marked as such.
- Lap validity comes for free (`currentLapInvalid`, Session History valid bits).
- Track/car identity: `trackId` enum (appendix), `formula`, `teamId`. Fine for cross-user comparison within F1 25.
- Positions: world X/Y/Z in metres, so track maps come for free.
- UDP works over the network, so it works through Proton on Linux, and from consoles to a PC client.
- Send rates per packet: from the official spec, in games/f1.md.
- Implementation source: the **official EA specs** "Data Output from F1 25" (v3, May 2025) and "Data Output from F1 25: 2026 Season Pack" (v1.2, Sep 2026), PDFs from the EA forum (© EA; read, not copied; kept outside the repository). Before we had them, a copy of the header in `github.com/P403n1x87/f1-packets` `data/spec.h` served as read-only reference. Community parsers (`f1-game-library-models-25`, `f1-game-packet-parser`, `open-f1-parser`) are not used (ARCHITECTURE §12.2).

---

### 1.3 Le Mans Ultimate (LMU)

LMU runs on the rFactor 2 engine. Historically all tools used the third-party **rF2 Shared Memory Map Plugin** DLL (`$rFactor2SMMP_Telemetry$`, `$rFactor2SMMP_Scoring$`, …). Track Titan and Racelab still install their own plugin DLLs. Since 2025, **LMU publishes its own shared memory**:

- **Map name `LMU_Data`**, a single `SharedMemoryObjectOut` struct (`#pragma pack(4)`), defined in the official `SharedMemoryInterface.hpp` in `<LMU>\Support\SharedMemoryInterface`.
- Requires **Settings → Gameplay → Enable Plugins = ON** (even with no plugin installed), and a game restart. With it off, the map doesn't exist.
- Layout:
  - `generic`: a queue of events (`SME_ENTER`, `SME_START_SESSION`, `SME_END_SESSION`, `SME_ENTER_REALTIME`, `SME_UPDATE_SCORING`, `SME_UPDATE_TELEMETRY`, …; the header's copy function treats the entries as event ids, not counters), game version, FFB torque, app state.
  - `paths`: UserData, results, player profile folder paths.
  - `scoring`: `ScoringInfo` (track name, session type, phase, weather, track length …) + `VehicleScoring[104]` (place, lap dist, sector/lap times best/last, pit state, class, flags, …).
  - `telemetry`: `activeVehicles`, `playerVehicleIdx`, `playerHasVehicle`, `VehicleTelemetry[104]`.
- **Vehicle telemetry fields** (from the rF2 `InternalsPlugin` heritage plus LMU additions):
  - Time/lap: `mElapsedTime`, `mDeltaTime`, `mLapNumber`, `mLapStartET`, `mCurrentSector`, `mLapInvalidated`, `mDeltaBest`
  - Position/dynamics: `mPos` (world, m), `mLocalVel`, `mLocalAccel`, `mOri[3]` (orientation matrix), `mLocalRot`, `mLocalRotAccel`
  - Inputs: unfiltered and filtered throttle/brake/steering/clutch, `mPhysicalSteeringWheelRange`, `mSteeringShaftTorque`
  - Engine: `mGear`, `mEngineRPM`, `mEngineMaxRPM`, water/oil temp, `mEngineTorque`, `mTurboBoostPressure`, `mFuel`, `mFuelCapacity`
  - Aero/chassis: front/rear ride height, front wing height, 3rd-spring deflection, drag, downforce F/R
  - **Electronics (new, in-cockpit values):** `mTC`, `mTCSlip`, `mTCCut`, `mABS`, `mMotorMap`, `mMigration`, front/rear anti-sway, `mRegen`, `mTCActive`, `mABSActive`, `mRearBrakeBias`
  - Hybrid/energy: `mBatteryChargeFraction`, electric motor torque/RPM/temps/state, `mStateOfCharge`, **`mVirtualEnergy`**, `mLiftAndCoastProgress`
  - Misc: tyre compound F/R (index + name), damage (`mDentSeverity`, last impact), gaps to car ahead/behind, `mTrackLimitsSteps`, vehicle model/class
  - Per wheel (`mWheels[4]`): suspension deflection, ride height, brake temp/pressure, tyre load, grip fraction, pressure (kPa), 3-zone tread temp + carcass + inner layer temps (Kelvin), wear, camber, toe, lateral/longitudinal forces and patch velocities, surface type, flat/detached
- **Synchronisation** (header version of 2026): the events `LMU_Data_HoldEvent` and `LMU_Data_DataEvent` signal a new frame (wait on hold, then data), and the game writes under a lock (`LMU_SharedMemoryLockData`: `waiters` + `busy`, event `LMU_SharedMemoryLockEvent`) that readers take while copying. Older builds have neither.
- **Lap distance** lives only in `VehicleScoring.mLapDist` (5 Hz). For high-rate distance you interpolate between scoring updates using `mElapsedTime` + speed, or compute arc length from `mPos` against a reference line. This is a common pitfall.
- **REST API** `http://127.0.0.1:6397` (OpenAPI at `/swagger-schema.json`, ~178 paths). Useful GETs: `/rest/sessions`, `/rest/watch/standings`, `/rest/strategy/usage` (Virtual Energy per driver), `/rest/garage/getPlayerGarageData` (setup values), `/rest/garage/UIScreen/RepairAndRefuel`, `/rest/garage/UIScreen/TireManagement`. Use GET only, and stay well away from the 100+ mutating endpoints. Poll at 1–5 Hz, off the hot path.
- Results XML files are written to `UserData\Log\Results` (path also exposed in `paths`). Useful for session summaries.
- Linux/Proton: Wine doesn't expose Windows named mappings to native processes, so a helper has to run inside the game's prefix (§2).
- Implementation source: the **official `SharedMemoryInterface.hpp`/`InternalsPlugin.hpp`** from the LMU install and LMU's own `/swagger-schema.json`. Third-party projects (TinyPedal/pyLMUSharedMemory, lmu-pitwall, race-engineer) were used only as sources of facts for this research (e.g. map name, required setting). We take no code from them (ARCHITECTURE §12.2).

---

## 2. Linux & Proton

What Stintory built is in [games/proton.md](games/proton.md). For adding games:

- **UDP-based games** (F1, also AMS2/PC2, ACC broadcast, Forza, GT7, …) work from Proton as they are, because the network is shared.
- **Shared-memory games** need a helper `.exe` inside the game's Wine prefix that reads the Windows mapping and forwards it. Stintory's `sy-bridge.exe` does this over localhost TCP and can be extended to more games. Other approaches looked at: `simshmbridge` (Linux `/dev/shm` file backing a mapping created in the prefix; AC/ACC, AMS2/PC2, LMU, rF2), `winecarte` (mirrors Win32 named mappings into `/dev/shm`) and `Datalink` (changes the Proton launch command to start a bridge that then launches the game).

---

## 3. Competitors – what they do and collect

As of November 2026. Prices and supported games change often; check before relying on them.

| | Garage61 | Track Titan | Trophi.ai | Coach Dave Delta |
|---|---|---|---|---|
| Sims | Primarily iRacing (a 2025 competitor comparison calls it iRacing-only; **verify** current list) | iRacing, LMU, AC, ACC, AC EVO, AMS2, Forza Motorsport, F1 2020–F1 25 (incl. consoles via UDP) | iRacing, ACC, F1 24/25, LMU (PC only) | iRacing, ACC, LMU, AC EVO, AC, AMS2, GT7 |
| Capture | Desktop agent reading iRacing SDK | Desktop app; LMU via own plugin `TrackTitanRf2Plugin.dll` | Desktop app | Desktop app |
| Analysis | Browser analyser: speed/throttle/brake/steering/gear/RPM traces vs distance, delta, line comparison, fuel/tyre stats, track map, ghost laps, **CSV export** | Traces (throttle/brake/speed/steering/gear), reference-lap comparison, AI tips, "coaching flows", real-time overlay | Live overlays + **real-time AI voice coach** ("Mansell"), turn-by-turn coach report, pro reference laps | Racing line/braking/throttle/gear overlay vs pro reference, "Auto Insights" (where time is lost), side-by-side video |
| Sharing model | **Laps & setups shared with all members by default**, opt-out per category; teams | Central database; compare vs other users/pros | Central; pro reference laps | Central; pro reference laps |
| Price | Free (last year of data), Pro ~€5/mo | Free tier + Pro/Premium | Subscription (~$18/mo) | ~£12/mo |

Other relevant tools:
- **iRacing × Cosworth Pi Toolbox**: free for iRacing subscribers since Sept 2025. Pro-level analysis software (with live remote telemetry announced).
- **MoTeC i2 / Race Engineering Center / Atlas**: offline analysis of `.ibt` (via converters such as "Mu").
- **SimHub, Racelab, TinyPedal, CrewChief**: overlays/dashes/spotter. Good references for multi-game data mapping (CrewChief is open source C#; TinyPedal is GPL Python).
- **Open-source self-hosted attempts**: *TelemetryStack* (iRacing `.ibt` → QuestDB → dashboard, Raspberry Pi), *RaceIQ* (local DB, F1 25/ACC/Forza/AC EVO), *omatrack* (distance-aligned lap comparison). **None covers our scope** of multi-game capture + local-first + self-hosted multi-user sync with per-lap sharing. That gap is what Stintory fills.

**What every competitor shows (feature baseline):**
1. Session list (track, car, date, conditions, best lap)
2. Lap list with lap/sector times, validity
3. Lap view: traces vs **distance** – speed, throttle, brake, steering, gear, RPM
4. Comparison of 2+ laps with **time delta trace** and track map with cursor sync
5. Reference laps (own best, others', "pros")
6. Export (CSV at least)

**What differentiates the paid ones:** AI/automated coaching (corner detection, braking-point/min-speed/throttle-pickup comparison per corner, natural-language tips), live voice coaching, setup marketplace, video sync. Stintory has corner insights, computed locally without an AI service ([ARCHITECTURE.md §6.5](ARCHITECTURE.md#6-analysis-sy-analysis)); the others are candidates for later.

---

## 4. Sources

- LMU native shared memory: [lmu-pitwall](https://github.com/Swizzjack/lmu-pitwall), [TinyPedal README](https://github.com/TinyPedal/TinyPedal/blob/master/README.md), [pyLMUSharedMemory](https://github.com/TinyPedal/pyLMUSharedMemory), [race-engineer LMU integration notes](https://github.com/Alexander-Gro/race-engineer/blob/main/docs/03-LMU-INTEGRATION.md), [Track Titan LMU setup](https://intercom.help/track-titan/en/articles/10775439-le-mans-ultimate-lmu-telemetry), [kerb crate](https://docs.rs/crate/kerb/0.1.2/source/README.md)
- F1: [EA F1 25 2026 Season Pack UDP spec (forum)](https://forums.ea.com/blog/f1-games-game-info-hub-en/ea-sports%E2%84%A2-f1%C2%AE25-2026-season-pack-udp-specification/12187347), [spec header copy in f1-packets](https://github.com/P403n1x87/f1-packets), [F1Laps 2026 UDP settings](https://www.f1laps.com/faqs/f1-26-udp-telemetry-settings/), [Motorsport.com – no F1 26](https://www.motorsport.com/f1/news/there-will-be-no-new-f1-26-game-ea-sports-announces/10777861/), [GTPlanet – 2026 Season Pack release](https://www.gtplanet.net/f1-25-2026-season-pack-out-now-with-new-madring-circuit-20260603/)
- iRacing: [goirsdk variable list](https://github.com/mpapenbr/goirsdk/blob/v0.6.0/variables.md), [iRacing Atlas quickstart (ibt)](https://ir-core-sites.iracing.com/members/atlas/atlas_quickstart.pdf), [teamjorge/ibt](https://github.com/teamjorge/ibt), [iRacing × Cosworth](https://www.onlineracedriver.com/2025/08/28/iracing-and-cosworth-partner-on-a-free-telemetry-system/)
- Linux: [simshmbridge](https://github.com/Spacefreak18/simshmbridge), [winecarte](https://github.com/srounce/winecarte), [awesome-linux-simracing](https://github.com/LukasLichten/awesome-linux-simracing)
- Competitors: [Track Titan supported games](https://intercom.help/track-titan/en/articles/6227990-what-games-are-supported-by-track-titan), [Coach Dave – Delta vs Garage 61](https://coachdaveacademy.com/tutorials/coach-dave-delta-vs-garage-61-which-is-best-for-you/), [Trophi.ai features](https://www.trophi.ai/sim-racing-features/pro-telemetry)
- Open source: [RaceIQ](https://github.com/SpeedHQ/RaceIQ), [TelemetryStack](https://github.com/OJPARKINSON/TelemetryStack), [omatrack](https://github.com/tobi/omatrack)
