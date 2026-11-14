# Stintory – Architecture

How Stintory is built and the rules it follows. Code comments cite this document by section (e.g. `docs/ARCHITECTURE.md §5.2`), so **keep the section numbers stable**: add new topics as new sections at the end or as sub-sections, and don't renumber.

See [RESEARCH.md](RESEARCH.md) for the telemetry interfaces and the competitor analysis these decisions are based on.

## 1. Key decisions

| Topic | Decision |
|---|---|
| Stack | **Rust** for capture, storage, analysis and server. **Tauri 2** desktop client with a **Svelte 5 + TypeScript** web frontend (SvelteKit, static SPA build) that is reused as the server's web UI |
| Capture platforms | iRacing and LMU capture on **Windows**, and on Linux for games in Proton through a small in-prefix helper (`sy-bridge.exe`, §3). F1 25 (UDP) on Windows and Linux, also from consoles |
| Linux client | Same client as on Windows: browse, compare, import/export, sync, `.ibt` import. Records F1 25 natively and Proton games through `sy-bridge.exe` |
| Server | Docker container: sync + sharing + **web UI** for browsing/comparing laps |
| Privacy | Client is fully offline by default. No analytics and no network calls except to the user-configured server |
| License | Open source. **AGPL-3.0-or-later** for apps (client, server, web UI), **MIT OR Apache-2.0** for reusable library crates (see §12) |
| Parsers | **Written from scratch** from official game specs/SDKs and our own recordings. No code from, and no dependency on, other telemetry tools (TinyPedal, CrewChief, SimHub, kerb, pyirsdk, …) |
| Recorded data | **Player's own car only.** Other cars' telemetry and other drivers' personal data (names, IDs, ratings) are never stored (§5.2) |
| Sharing | Users create **teams** and invite others. Sessions/laps can be shared with **individual users, teams and/or everyone on the server**. Per-user default plus per-session/lap override (§9.2) |
| Export format | `.stintory` bundles (§8) |
| Login | **Username + password**, **passkeys** (WebAuthn) and **TOTP** 2FA. OIDC later (§9.3) |
| Raw data | Raw game data is spooled to disk and **deleted once processed**. A debug setting keeps the raw files (§5.1) |
| AI | No built-in AI service. The user's own assistant reads laps and the corner analysis over MCP: from the server (`/mcp`) or from the desktop app (`stintory mcp`) (§6.5) |

## 2. Scope

**Core features** (all built, see §11)
- Automatically record every session/lap while driving, with no clicks needed
- Session and lap browser, lap view with traces over distance, **compare two laps** (delta trace, track map, synced cursor)
- Export selected sessions/laps to a file; import files (own or other people's)
- Optional sync to a self-hosted server; teams; share with users/teams/everyone, with a per-user default plus per-session/lap override
- Secure login: password, passkeys, TOTP
- Web UI on the server with the same analysis views

**Not built yet** (candidates, see §11): live overlays/HUD, voice coaching, built-in AI tips (an external assistant can use the MCP endpoint, §6.5), setup sharing, video sync, mobile app, OIDC login.

**Never:** recording other cars or other drivers' data.

## 3. Architecture

```
 Windows PC (driving)                                     Self-hosted server (Docker)
┌───────────────────────────────────────────────┐        ┌──────────────────────────────┐
│ Game ──shm/UDP──► Source adapter (sy-capture) │        │ sy-server (axum)             │
│                    │ normalized Frames         │  HTTPS │  REST API  /api/v1/...       │
│                    ▼                           │◄──────►│  auth, sharing, sync         │
│               Recorder / lap segmenter         │  sync  │  PostgreSQL (metadata)       │
│                    │ Laps (immutable)          │        │  blob store (FS / S3)        │
│                    ▼                           │        │  serves web UI (same TS app) │
│      Local store: SQLite + Parquet blobs       │        └──────────────────────────────┘
│                    ▲                           │                    ▲
│  Tauri backend (sy-analysis, sy-store)         │                    │ browser
│                    ▲ invoke()                  │             Any device (Linux …)
│  Web frontend (TS): sessions, lap view, compare│
└───────────────────────────────────────────────┘
 Linux PC: same client. F1 25 over UDP; shared-memory games in Proton through the bridge:
 Proton:          game ─shm─► sy-bridge.exe (in prefix) ─localhost TCP─► client
```

Key idea: **the frontend talks to one API shape** (`ApiClient` interface). In the desktop app it calls the Tauri backend (`invoke`). In the browser it calls the server over HTTP. Both backends use the same Rust crates (`sy-model`, `sy-analysis`), so resampling, delta computation, etc. exist exactly once.

### 3.1 Repository layout (monorepo)

```
Cargo.toml                 # workspace
crates/
  sy-model/                # domain types: Session, Lap, Channel, Units, IDs, (de)serialization
  sy-capture/              # Source trait, raw spool + crash recovery, player-only filter hooks, recorder/lap segmenter
  sy-source-iracing/       # shm reader (cfg windows) + .ibt parser (all platforms)
  sy-source-f1/            # UDP parser 2025/2026 formats (all platforms)
  sy-source-lmu/           # LMU_Data reader (cfg windows) + REST poller
  sy-store/                # local SQLite + Parquet blob store, export/import bundles
  sy-analysis/             # distance resampling, delta, track map, sectors, corner insights
  sy-sync/                 # sync client (HTTP)
  sy-server/               # axum server binary
  sy-bridge/               # Proton bridge: sy-bridge.exe (in the game's prefix) + the app's client
  sy-mcp/                  # MCP tools + protocol for AI assistants (server /mcp, desktop `stintory mcp`)
apps/
  desktop/                 # Tauri 2 app (src-tauri/ + uses web/ build)
web/                       # SvelteKit app (adapter-static, SPA mode), shared by desktop + server
  src/lib/api/             # ApiClient interface + TauriApi + HttpApi
  src/lib/charts/          # uPlot wrapper components, track map canvas
  src/lib/components/      # shared UI components (tables, dialogs, share picker …)
  src/routes/              # sessions, session detail, lap view/compare, teams, account, settings
docker/                    # Dockerfile, docker-compose.yml example
fixtures/                  # recorded raw telemetry for tests (see §10)
docs/
LICENSE                    # AGPL-3.0 (default for the repo)
LICENSES/                  # REUSE-style full texts: AGPL-3.0-or-later, MIT, Apache-2.0
```

### 3.2 Technology choices

| Concern | Choice | Why |
|---|---|---|
| Shared memory (Windows) | `windows` crate: `OpenFileMappingW`, `MapViewOfFile`, `OpenEventW`, `WaitForSingleObject` | Zero-copy, event-driven (no busy polling for iRacing) |
| UDP | `tokio::net::UdpSocket` | Simple, async |
| Binary parsing | `zerocopy`/`bytemuck` for packed structs; hand-written for iRacing's self-describing var table | Safe and fast |
| Local DB | SQLite via `rusqlite` (bundled) | Single file, no service |
| Telemetry blobs | **Parquet** (`arrow`/`parquet` crates, zstd compression), one file per lap | Columnar + compressed; users can open their data in pandas/DuckDB/Excel-ish tools ("it's my data") |
| Server | `axum` + `tokio-postgres`/`deadpool-postgres` + **PostgreSQL** (own embedded SQL migrations), blobs on local volume (S3-compatible optional) | Standard, easy Docker deploy. `sqlx` was dropped: its SQLite driver pins an older `libsqlite3-sys` than the client's `rusqlite`, and Cargo resolves that conflict even with the feature off |
| Auth | `argon2` (password hashes), `webauthn-rs` (passkeys, pure Rust since 0.6), own RFC 6238 TOTP (tested against the RFC vectors), cookie sessions for the web UI, per-device API tokens for clients. Later `openidconnect` | Self-hosters often have no IdP. Passkeys + TOTP give strong security without one |
| Frontend | **Svelte 5** (runes) + TypeScript, **SvelteKit** with `adapter-static` (SPA, no SSR), Vite, **uPlot** for traces, Canvas for track map, shadcn-svelte/Bits UI for UI components, TanStack Table for lap tables. Tests: Vitest + Playwright | Little boilerplate, easy to learn, built-in reactivity/state. A static build runs unchanged in Tauri and served by axum. uPlot handles 100k+ points smoothly with cursor sync across charts |
| Packaging | Tauri bundler: MSI/NSIS (Windows), AppImage/deb/rpm (Linux). Server: multi-stage Docker image | |
| CI | Forgejo Actions on an Ubuntu runner (`.forgejo/workflows/ci.yml`): `cargo fmt`/`clippy`/`test`, frontend check/lint/tests/build, `reuse lint`, `cargo deny`, license-direction script. Windows builds and Windows-only tests run locally for now; CI also compile-checks the Windows-only crates against the Windows targets and cross-builds `sy-bridge.exe` with MinGW | License checks keep the license split (§12) enforced |

## 4. Data model

### 4.1 Entities

```
Driver        id, display_name, game_identities{iracing_cust_id, f1_name, lmu_name}
Session       id (UUIDv7), game, game_version, track_key, layout_key, car_key,
              session_type (practice/qualy/race/time_trial/hotlap), started_at, ended_at,
              weather/track_temp/air_temp, setup (JSON), session_info (JSON, filtered to player only, §5.2),
              origin (own | imported:<bundle> | synced:<server>/<user>), sharing (see §9.2)
Lap           id (UUIDv7), session_id, number, lap_time_ms, sector_times_ms[],
              valid (bool) + invalid_reason, flags (out_lap, in_lap, pit, flashback, incident),
              tyre/fuel at start, blob_hash (sha256 of Parquet file), sample_rate_hz,
              sharing (see §9.2), notes, tags
Track         game, track_key, layout_key, display_name, length_m, sector boundaries, outline (polyline)
Car           game, car_key, display_name, class
RawSpool      (client only) id, game, started_at, path, state (recording | processing | processed | failed), keep (bool)
```

Server-only entities (users, teams, invites, share grants, credentials) are in §9.

- **IDs are generated on the client** (UUIDv7), so sync is conflict-free. Lap telemetry is **immutable** and content-addressed (`blob_hash`). Only small metadata fields (sharing, notes, tags) are mutable (last-write-wins with `updated_at`).
- **Canonical keys** make laps comparable across users: `iracing:<TrackID>:<config>`, `iracing:<CarID>`; `f1_25:<trackId>`, `f1_25:<teamId>/<formula>`; `lmu:<track name normalized>:<layout length m>`, `lmu:<vehicle model>:<class>`.

### 4.2 Channels (per lap, columnar)

Every adapter maps game data to **canonical channels in SI units**. It also keeps useful game-specific extras under a namespace. Comparison works on canonical channels; extras are shown when both laps have them.

| Canonical channel | Unit | iRacing | F1 | LMU |
|---|---|---|---|---|
| `t` | s since lap start | SessionTime − lap start | sessionTime − lap start | mElapsedTime − mLapStartET |
| `dist` | m from S/F line | `LapDist` | `m_lapDistance` | interpolated `mLapDist` (scoring) |
| `speed` | m/s | `Speed` | `m_speed`/3.6 | ‖`mLocalVel`‖ |
| `throttle`, `brake`, `clutch` | 0–1 | `Throttle`, `Brake`, 1−`Clutch` (check sense) | as-is, clutch/100 | unfiltered inputs |
| `steer` | rad at wheel, + = right (verify sign per sim) | `SteeringWheelAngle` | `m_steer` × lock (unknown → store normalized `steer_norm`) | `mUnfilteredSteering` × range/2 |
| `gear`, `rpm` | –, rpm | `Gear`, `RPM` | `m_gear`, `m_engineRPM` | `mGear`, `mEngineRPM` |
| `pos_x/y/z` | m world | dead-reckoned or `Lat/Lon/Alt` (ibt) | `m_worldPosition*` | `mPos` |
| `g_lat/g_long/g_vert` | m/s² | `LatAccel`… | `m_gForce*`×9.81 | `mLocalAccel` |
| `yaw/pitch/roll`, `yaw_rate` | rad, rad/s | `Yaw`… | `m_yaw`… | from `mOri`, `mLocalRot` |
| `fuel` | l (or kg for F1) | `FuelLevel` | `m_fuelInTank` | `mFuel` |
| `brake_bias` | % front | `dcBrakeBias` | `m_frontBrakeBias` | 1−`mRearBrakeBias` |
| `abs_active`, `tc_active` | bool | `BrakeABSactive` | – | `mABSActive`, `mTCActive` |
| tyres `tyre_temp_{fl..rr}`, `tyre_press_*`, `brake_temp_*` | °C, kPa | where live | `m_tyresSurfaceTemperature`… | `mWheels[]` (K→°C) |
| extras | | `iracing.*` e.g. shock defl, `dcTractionControl` | `f1.ers_*`, `f1.drs`, `f1.active_aero`, `f1.overtake` | `lmu.virtual_energy`, `lmu.battery`, `lmu.tc_level`, `lmu.abs_level`, `lmu.motor_map`, ride heights, per-wheel tyres/brakes |

Storage: samples at the **native rate** (iRacing 60 Hz, F1 = configured rate, LMU ~100 Hz). Resampling happens only in analysis. Estimated size: 60 Hz × ~40 channels × 4 B ≈ 10 KB/s raw, so a 90 s lap ≈ 0.9 MB raw and **~150–300 KB as Parquet+zstd**. One hour ≈ 6–10 MB. Fine for years of local data.

## 5. Capture pipeline

The adapter interface lives in `sy_model::capture` (MIT/Apache), so the permissively licensed game crates can implement it. The recorder and the spool are in `sy-capture` (AGPL).

```rust
trait RawSource {                 // per game: shared memory, UDP, file
    fn game(&self) -> GameId;
    fn next_record(&mut self, timeout: Duration) -> Result<Option<RawRecord>>;  // already player-only (§5.2)
}
trait Decoder {                   // per game: raw bytes -> events
    fn decode(&mut self, record: &RawRecord) -> Result<Vec<SourceEvent>>;
    fn finish(&mut self) -> Vec<SourceEvent>;
}
enum SourceEvent {
    Session(SessionMeta),         // new session (new key) or changed info (same key)
    Frame(Frame),                 // one sample of the player's car
    LapTime { lap, lap_time_ms, sector_times_ms },  // official time, may arrive after the boundary
    Invalidate { reason },        // current lap invalid
    Rewind { to_session_time },   // F1 flashback
    LapAborted { reason },        // tow, reset, left the car
    SessionEnd,
}
```

```text
live:      RawSource ─► spool file (.syraw) ─┐
                     └──────────────────────► Decoder ─► Recorder ─► Store
recovery:  spool file ─────────────────────► Decoder ─► Recorder ─► Store
import:    file RawSource (.ibt) ──────────► Decoder ─► Recorder ─► Store
```

- **Supervisor** (in the Tauri backend, background thread): probes all enabled sources every ~2 s. When one becomes active, it runs the source loop until the game is gone. Tray icon shows status ("Recording iRacing – Spa – lap 7").

### 5.1 Raw spool (raw data kept until processed)

Pipeline: `game → reader → raw spool file (append) → parser → recorder → Parquet + SQLite`.

- Every raw sample the reader receives is first appended to a **spool file** `<data>/raw/<game>-<session-uuid>.syraw`. The format is a small header + length-prefixed, timestamped records, compressed in zstd frames so a crash only loses the last frame.
  - iRacing: the var header table (once and on change), the player-filtered session YAML (on change) and one data buffer per tick.
  - F1: UDP datagrams (filtered, §5.2).
  - LMU: only the `generic` block, the player's `VehicleTelemetry`, `ScoringInfo` and the player's `VehicleScoring` per update. Not the whole ~MB `LMU_Data` map.
- The parser consumes the live stream as usual. A spool file's state becomes `processed` once the session has ended **and** every lap from it is committed (Parquet written + SQLite row). Then the file is **deleted**.
- **Crash recovery:** at startup, spool files in state `recording`/`processing` are replayed through the same parser + recorder (dedup by lap ID). After a crash or power loss, nothing is lost, not even the lap in progress.
- `failed` (parser error): the file is kept and the UI shows "1 recording could not be processed", with an option to retry after an update or delete it. This is very useful after game updates that change layouts.
- **Debug setting "Keep raw recordings"** (Settings → Advanced/Debug, off by default): processed spool files are kept instead of deleted. The UI shows folder size, an "open folder" button and a "delete all raw files" button. Raw files are **never synced and never exported** in `.stintory`. A user can attach them manually to a bug report.
- Size (measured): iRacing ≈ 60 rows/s × ~1.1 KB. With zstd that comes to **~73 MB per hour** (18.7 MB for a 15-minute race). Only matters when "keep" is on, because processed spools are deleted.
- `.ibt` import: the user's `.ibt` file is the raw source. It is read in place and never modified or deleted.

### 5.2 Player car only

- Readers extract **only the player's car**. iRacing: the `PlayerCarIdx` entries. F1: `playerCarIndex` from the header. LMU: `playerVehicleIdx` / the scoring entry with `mIsPlayer`.
- Data about other cars and drivers is removed **before it reaches disk**, including the spool:
  - iRacing: `CarIdx*` arrays are zeroed except for the player's index. The session YAML's `DriverInfo.Drivers` and results are reduced to the player.
  - F1: other cars' entries in per-car arrays are zeroed. Per-car packets for other cars (Session History, Tyre Sets) are dropped. Participants/Lobby are reduced to the player.
  - LMU: only player structs are spooled (see above).
- Session-level information that isn't about other drivers stays (track, weather, session type, field size, own finishing position).
- Tests assert this: a fixture recorded with other cars must produce stored data that contains no other drivers' names or telemetry.
- **Recorder / lap segmenter** (game-independent state machine):
  - Starts a new lap buffer when `dist` wraps (large negative jump near track length) **and/or** the game's lap counter increments. The game's official lap time wins over our measured time.
  - Marks out-laps (started in pits), in-laps (ending in pits), incomplete laps (session end, tow, disconnect). These are stored but flagged.
  - Validity: F1 and LMU flags directly. iRacing: incident counter increase, `PlayerTrackSurface` off-track, or pit road during the lap → invalid with a reason.
  - Rewind: truncate buffer to `to_time`, flag lap `flashback`.
  - On lap completion: build Arrow batch → Parquet → hash → write blob → insert Lap row (one transaction). Combined with the raw spool (§5.1), a crash loses nothing.
- **iRacing specifics:** wait on `IRSDKDataValidEvent`. Parse the var table once per session (re-parse if the header changes). Parse the session YAML on `sessionInfoUpdate` change. Player car only. `.ibt` import uses the same var-table parser plus the same recorder (file source instead of shm source).
- **F1 specifics:** bind UDP port (configurable, default 20777; this PC only, or all interfaces for consoles/other PCs). Group by `sessionUID` + player car index. One Frame per Car Telemetry packet with the latest Motion, LapData, Status, MotionEx and Telemetry2. Branch on `packetFormat` 2025/2026. Player index from header. Optional UDP forwarding to other tools (many users run several F1 apps). Details: docs/games/f1.md.
- **LMU specifics:** open `LMU_Data`. Wait for a frame on the events `LMU_Data_HoldEvent` + `LMU_Data_DataEvent` and copy the player's structs under the game's lock `LMU_SharedMemoryLockData` (bounded wait), as the current `SharedMemoryInterface.hpp` describes (`generic.events` turned out to be an event queue, not counters). Interpolate lap distance from 5 Hz scoring. Virtual energy, TC/ABS and motor map are in the shared memory now; the REST API (a fixed list of GET endpoints) gives the car setup and tells "game running without shared memory" apart. Show a clear hint in the UI if the map is missing ("Enable Plugins in LMU Gameplay settings and restart"). Details: docs/games/lmu.md.

## 6. Analysis (`sy-analysis`)

1. **Distance resampling**: resample each lap onto a fixed distance grid (e.g. every 1 m, or 0.5 m for short tracks) with linear interpolation (step for gear/booleans).
2. **Time delta**: Δt(d) = t_A(d) − t_B(d). Also the delta's derivative to highlight where time is gained/lost.
3. **Track map**: from `pos_x/y` (F1/LMU) or dead reckoning/GPS (iRacing). Store a smoothed outline per track (median of several clean laps) in `Track.outline`. Colour by delta/speed/throttle/brake.
4. **Sectors**: game sectors. Plus user-definable micro-sectors (later).
5. **Corner insights** (local, no AI service; `sy_analysis::corners`): corners come from the reference lap's curvature `g_lat / speed²` (smoothed, radius < 250 m, same-direction bends < 30 m apart merged, a bend inside the next corner's braking zone belongs to that corner). Per corner and lap: braking point, peak pressure and release, lift, lowest speed and gear, throttle pick-up, full throttle, exit speed, and the time lost, split into entry (to the apex) and exit. The lap is split into one segment per corner (approach → next approach, wrapping around the line), so the times lost add up to the gap between the laps. Checked on the iRacing, F1 and LMU recordings (e.g. Hungaroring 14, Suzuka 18, Snetterton 300 11 corners). Shown in the compare view (corner list sorted by time lost, corner numbers on the map) and part of `CompareData`. AI assistants read it through the server's MCP endpoint (`/mcp`, docs/SERVER.md): OAuth 2.1 (own authorization server, client ID metadata documents or dynamic registration, PKCE, resource-bound access tokens, rotating refresh tokens) or read-only device tokens; tools for tracks, sessions, best laps, corner comparison and telemetry excerpts. Without a server, the desktop app serves the same tools from the local data: `stintory mcp` speaks MCP over stdio for assistants on the same computer (Settings → AI assistants shows the command). The tools and the protocol live once in `sy-mcp` behind a small data trait.
6. **Theoretical best**: sum of best sectors (game sectors or micro-sectors).

Frontend gets resampled arrays (not raw) for comparison. These are computed on demand and cached in memory (LRU).

## 7. Client UI (desktop and server web UI share it)

1. **Dashboard / Recorder status**: which games are detected, current session, last laps.
2. **Sessions list**: filter by game/track/car/date/type; best lap; laps count; sharing badge; origin (own/imported/synced).
3. **Session detail**: lap table (time, sectors, valid, flags, fuel, tyres); select laps for comparison; share dialog; notes/tags.
4. **Lap view / Compare**: stacked traces vs distance (speed, throttle/brake, steering, gear, RPM, delta). Track map with synced cursor. Zoom/pan synced across charts. Choose "reference" = own best / any lap / imported / lap shared with me.
5. **Library of references**: laps from imports and laps shared with me (by users or teams) for the same track+car.
6. **Import/Export**: select sessions/laps → export bundle. Drag & drop `.stintory` / `.ibt` to import.
7. **Share dialog** (session or lap): "Use my default" or custom: an "Everyone on this server" toggle plus a picker for users and teams, with a summary of who can see it.
8. **Teams** (when connected to a server): my teams, create team, invite (by username or invite link), pending invites to accept/decline, members and roles, leave team.
9. **Account** (when connected): login via browser (§9.3), change password, manage passkeys, set up TOTP + recovery codes, list/revoke devices.
10. **Settings**: data directory, enabled games, F1 UDP port/forwarding, server URL, default sharing, auto-sync on/off. **Advanced/Debug:** "Keep raw recordings" + raw folder size/open/delete, log level.

## 8. Export / import format

`*.stintory` = ZIP (implemented in `sy-model::bundle`, MIT OR Apache-2.0):
```
manifest.json        format "stintory-export", format_version "1.0", exported_at, exporter, driver_name (optional), counts
sessions/<id>.json   session metadata (incl. track/car keys)
laps/<id>.json       lap metadata
laps/<id>.parquet    telemetry (same file as in the local store, stored uncompressed in the ZIP; hash verified on import)
```
- Import dedups by session and lap id: what is already stored stays as it is. New sessions get `origin = imported` with the driver name from the file (a re-exported imported session keeps its original driver).
- Imported sessions are never merged with own sessions by the "same session" rule (§5, `find_duplicate_session` only matches sessions of the same origin). Session keys contain the driver's game id, so two drivers in the same online session get different ids.
- Privacy on export: the user picks the driver name (empty = anonymous, remembered for the next export). Setups are left out unless chosen. The game account name and id are removed from the session info.
- Additional exports: **CSV** per lap (for spreadsheets; header `name [unit]`, empty = no value), later **MoTeC `.ld`** (for i2 users).
- `format_version` is checked; newer minor versions are readable, unknown fields are ignored. Entry sizes are limited on import (ZIP bombs).

## 9. Server

### 9.1 Responsibilities
- User accounts and authentication (§9.3), server admin role
- Receive synced sessions/laps (metadata + Parquet blobs), dedupe by hash
- Teams, invitations and sharing (§9.2)
- Query API: list own sessions, list laps shared with me by track+car (leaderboard style), fetch lap metadata/blob/resampled data
- Serve the web UI (static build of `web/`) and the same analysis endpoints as the Tauri backend

### 9.2 Teams & sharing

**Entities (server)**
```
User           id, username (unique), display_name, is_admin, created_at, disabled
Team           id, name, created_by, created_at
TeamMember     team_id, user_id, role (owner | admin | member), joined_at
TeamInvite     id, team_id, invited_by, kind (user: invitee_id | link: token_hash, max_uses, uses),
               expires_at, status (pending | accepted | declined | revoked | expired)
ShareGrant     object_kind (session | lap | user_default), object_id, grantee_kind (user | team | everyone), grantee_id (null for everyone)
Sharing mode   on Session and Lap: inherit | custom
```

**Teams**
- Any user can create a team and becomes its **owner**. Owners and admins can invite, remove members and change roles (admins can't touch owners). Owners can rename/delete the team and transfer ownership. Every team keeps at least one owner.
- Invite **by username** (shows up as a pending invite in the invitee's client/web UI, accept/decline) or **by invite link** (random token, stored hashed, with expiry and optional max uses; the user must be logged in to accept).
- Members can leave a team at any time.

**Sharing**
- Audience = any combination of **users, teams and "Everyone on this server"**. An empty set means private. "Everyone" covers all active accounts, including ones created later.
- Server setting `allow_share_everyone` (default on): admins of larger or public servers can disable the "Everyone" option. Existing "everyone" grants then stop counting, and the UI shows those items as "not shared (everyone disabled by admin)".
- Resolution: a lap with mode `custom` uses its own grants. A lap with `inherit` uses its session's. A session with `inherit` uses the owner's **default grants** (`user_default`). New users default to private.
- The default only applies to sessions the owner **recorded**. Imported sessions (someone else's `.stintory` file, usually) stay private until they are shared explicitly.
- Access check: user U may read a lap if U is the owner, or U is directly granted, or U is a member of a granted team, or there is an `everyone` grant (and it's allowed on the server). Team access is evaluated **live**: whoever joins later sees team-shared laps, and whoever leaves loses access. A user can only share with teams they are in; when they leave a team, what they shared with it stops being shared.
- A session is visible to others through its readable laps: a private session with one shared lap shows up with that lap only.
- Changing the default affects everything still on `inherit`. The UI says so explicitly ("applies to 37 sessions using your default").
- Sharing grants read access to session/lap metadata and telemetry only. Others can never edit, re-share or delete. The car setup and the game's session info are not part of what others get.
- Revocation: clients that cached a foreign lap (used as a reference) get the lost-access list on their next sync (`POST /shared/check`) and delete the cached copy. Disconnecting the app from the server deletes all cached copies, because access can't be checked anymore. Copies already exported to `.stintory` can't be recalled; the share dialog mentions this.
- User lookup for the picker: exact username, or autocomplete among people you share a team with. There is no full user directory, so usernames on a server aren't enumerable.

### 9.3 Authentication

**Methods**
- **Username + password**: argon2id hashes, minimum length (NIST-style: length over complexity rules), login rate limiting per account + IP, generic error messages.
- **TOTP** (RFC 6238, 30 s, 6 digits) as a second factor. Setup with QR code + confirm code. 10 single-use **recovery codes** (stored hashed). Secrets are encrypted at rest with a server key from the environment.
- **Passkeys** (WebAuthn, `webauthn-rs`): usable for **passwordless login** (discoverable credentials) or as a second factor. A user can register several passkeys and remove them. Requires HTTPS and a stable origin: the RP ID = the server's domain from config (`localhost` works for testing).
- A user must keep at least one usable login method. Admin can enforce 2FA (TOTP or passkey) for all users.
- Account recovery: an admin can reset a user's password and 2FA. CLI command `sy-server admin reset-user <name>` for the very first admin.
- **OIDC (later):** "Login with Authentik/Keycloak/…" via `openidconnect`, linking the external identity to a user.

**Web UI sessions:** server-side sessions in an `HttpOnly`, `Secure`, `SameSite=Lax` cookie, CSRF token on state-changing requests, session list with "log out other sessions".

**Desktop client login: via the system browser.** The Tauri webview on Linux (WebKitGTK) has unreliable WebAuthn support, and we want every method, including OIDC later, to work the same way.
1. The client opens `https://server/auth/device?...` in the default browser, using loopback redirect + PKCE (the pattern OAuth uses for native apps, RFC 8252).
2. The user logs in there with password/TOTP/passkey.
3. The server redirects to `http://127.0.0.1:<port>/callback?code=…`, and the client exchanges the code for a **device token**.
4. Device tokens are long-lived, stored hashed on the server and in the OS keychain on the client (`keyring` crate: Windows Credential Manager / Secret Service on Linux). They are listed in the account page and can be revoked individually.

Fallback for headless/unusual setups: create a device token in the web UI and paste it into the client.

**Security basics:** HTTPS required (reverse proxy) except for `localhost`. Audit log of security events (logins, 2FA changes, token creation/revocation, team membership changes).

### 9.4 API sketch (`/api/v1`)
```
# auth
POST /auth/login                 {username, password} → session | {second_factor: [totp, passkey]}
POST /auth/login/totp            {code | recovery_code}
POST /auth/passkey/login/start   POST /auth/passkey/login/finish
GET  /auth/device                (browser page, loopback + PKCE)   POST /auth/device/token {code, verifier} → device token
POST /auth/logout
# account
GET  /me   PATCH /me   POST /me/password
POST /me/totp/setup  POST /me/totp/confirm  DELETE /me/totp  POST /me/recovery-codes
POST /me/passkeys/register/start  POST /me/passkeys/register/finish  GET /me/passkeys  DELETE /me/passkeys/{id}
GET  /me/tokens  POST /me/tokens  DELETE /me/tokens/{id}   GET /me/sessions  DELETE /me/sessions/{id}
GET  /me/default-sharing  PUT /me/default-sharing   {grants:[{user:id} | {team:id} | {everyone:true}]}
GET  /users/lookup?username=…    GET /users/suggest?q=…  (team co-members only)
# teams
GET/POST /teams   GET/PATCH/DELETE /teams/{id}
GET /teams/{id}/members   PATCH/DELETE /teams/{id}/members/{user}   POST /teams/{id}/leave
POST /teams/{id}/invites {username} | {link: {expires_at, max_uses}}   DELETE /teams/{id}/invites/{inv}
GET /me/invites   POST /invites/{id}/accept|decline   POST /invites/link/{token}/accept
# data & sync
POST /sync/manifest              client sends [{id, kind, updated_at, hash}] → server answers what it needs (+ revoked foreign laps)
PUT  /blobs/{sha256}             upload Parquet (idempotent)
PUT  /sessions/{id}  PUT /laps/{id}           upsert metadata
GET/PUT /sessions/{id}/sharing  GET/PUT /laps/{id}/sharing   {mode: inherit|custom, grants:[…]} → also returns effective audience
GET  /sessions?game&track&car&mine=true
GET  /laps/{id}  GET /laps/{id}/blob  GET /laps/{id}/resampled?step=1
GET  /shared/sessions   GET /shared/sessions/{owner}/{id}   GET /shared/sessions/{owner}/{id}/download   POST /shared/check
GET  /shared/tracks             tracks + cars with laps shared with me (count, fastest, my best)
GET  /reference-laps?track=…&car=…&scope=all|mine|shared   (fastest first, ≤10 per driver; track keys contain the game)
POST /analysis/compare  {laps:[a,b], step}  → resampled channels + delta
# admin
GET/POST /admin/users  PATCH /admin/users/{id} (disable, reset password/2FA, admin flag)  GET/PUT /admin/settings (registration mode, enforce 2FA, allow_share_everyone)
```

### 9.5 Sync protocol (client side, `sy-sync`)
- Push: on lap completion (if auto-sync) or manually. Send manifest diff → upload missing blobs → upsert metadata. Retries with backoff and works offline-first (outbox table in SQLite).
- Pull: own data from other devices (e.g. view on Linux what was recorded on Windows), and **on-demand** fetch of laps shared with me when chosen as a reference (no full mirror). Teams and invites are refreshed on each sync. Cached foreign laps that lost access are deleted.
- Teams and sharing live on the server. The desktop app's frontend uses the same API as the browser; the Tauri backend passes the requests on with the device token (only the teams and sharing paths). So they need a connection for now.
- Later: sharing changes made offline go into the outbox and apply on the next sync. The picker offers users/teams from the last cached list.
- Mutable fields (sharing, notes, tags): last-write-wins on `updated_at`.
- Deleting: tombstones synced both ways. Deleting on the server removes the blob when no other reference exists.

### 9.6 Deployment
- `docker-compose.yml`: `stintory-server` + `postgres`. Volume for blobs. Env vars for public base URL (= WebAuthn origin/RP ID), secret key (TOTP encryption, cookies), admin bootstrap, registration mode (closed / invite-only / open).
- TLS via the user's reverse proxy (Caddy/Traefik/nginx). Documented, not built in. It is required for passkeys and secure cookies.
- Note in docs: the server admin can technically read all data on their server. That is fine for self-hosting, but we state it.

## 10. Testing & development strategy

Developing on Linux without the games running is a real constraint, so **record once, replay forever**:
- The **raw spool files** (§5.1) are the fixture format. Turn on "Keep raw recordings" on Windows, drive a few laps, and copy the `.syraw` files.
- **Replay sources**: one per game, feeding `.syraw` files through the exact same parser + recorder. Crash recovery uses the same code path. These are used in unit/integration tests and for UI development on Linux.
- Privacy test: a fixture recorded with other cars on track → stored data contains only the player (§5.2).
- Server tests: access-control matrix (owner / direct grant / team member / ex-member / everyone grant with the server setting on and off / stranger × inherit/custom at session and lap level), invite flows, auth flows (password, TOTP, recovery code, passkey via a software authenticator in tests).
- Fixtures in `fixtures/` (small, a few laps each). Larger ones via Git LFS or a download script.
- `.ibt` sample files also serve as iRacing fixtures.
- Golden tests: fixture → expected lap count, lap times (must match the game's official times within 1 ms), validity flags.
- Frontend: component tests + Playwright smoke test against the server with seeded data.

## 11. Status and roadmap

The MVP is complete (version 0.2.0, November 2026): the core features of §2 work for iRacing, F1 25 and Le Mans Ultimate, on Windows and Linux. The milestones that got there (M0 skeleton … M7 Proton bridge) are in the git history. From here on, Stintory grows through new features and refinements of existing ones, tracked as issues.

**Built**

| Area | What works |
|---|---|
| Capture | iRacing live (Windows) and `.ibt` import (everywhere). F1 25 incl. the 2026 Season Pack format over UDP (Windows, Linux, consoles/other PCs, forwarding to other apps). LMU from the shared memory (Windows, and Linux through `sy-bridge.exe`). Raw spool with crash recovery, player-only filtering before the spool. All three games checked with real recordings, which are golden tests (docs/games/) |
| Analysis | Distance resampling, delta, track map, game sectors, corner insights (§6.5) |
| Desktop app | Sessions, session detail, lap view, compare with synced traces and map, reference laps (own, imported, shared), `.stintory` + CSV export/import, settings for every game, Proton bridge setup, MCP over stdio (`stintory mcp`) |
| Server | Docker image; accounts with password, TOTP + recovery codes and passkeys; admin page and CLI reset; device login from the desktop app; sync of own data; teams, invites and sharing; web UI with the same views as the desktop app; reference laps across users; MCP endpoint with OAuth 2.1 (docs/SERVER.md) |

**Known gaps**
- Sharing changes while offline (outbox, §9.5): the desktop app needs a server connection for teams and sharing.
- Proton bridge: the open points under *To verify with the real game* in docs/games/proton.md.

**Ideas for later** (no fixed order): OIDC login (§9.3), micro-sectors and theoretical best (§6), MoTeC `.ld` export (§8), setup sharing, static track outlines (e.g. from LMU's REST track map, docs/games/lmu.md), live overlay, more games (ACC, AC EVO, AMS2, rF2, Forza …) by demand.

## 12. Licensing & clean-room parsers

### 12.1 License split

| Component | License | Reason |
|---|---|---|
| `sy-server`, `apps/desktop`, `web/`, `sy-capture`, `sy-store`, `sy-sync`, `sy-analysis`, `sy-bridge`, `sy-mcp` | **AGPL-3.0-or-later** | Anyone who changes these and distributes them, or runs them as a network service, must publish their changes. Nobody can turn Stintory into a closed hosted service |
| `sy-model` (incl. `.stintory` format), `sy-source-iracing`, `sy-source-f1`, `sy-source-lmu` | **MIT OR Apache-2.0** | Other tools can use the parsers and read/write the export format. That helps data portability and the format becomes a de-facto standard |

Rules:
- Permissive crates must **never depend on AGPL crates**. Enforced with `cargo deny` (the dependency direction model/sources → capture → store/analysis → apps makes this natural).
<!-- REUSE-IgnoreStart -->
- Every source file has an SPDX header (`// SPDX-License-Identifier: AGPL-3.0-or-later` or `MIT OR Apache-2.0`). Each crate sets `license` in `Cargo.toml`. `reuse lint` runs in CI.
<!-- REUSE-IgnoreEnd -->
- Contributions under the **DCO** (`Signed-off-by:` in commits), no CLA. Contributions to a crate are licensed under that crate's license.
- Third-party dependencies must be AGPL-3-compatible (MIT, Apache-2.0, BSD, ISC, MPL-2.0, Zlib…). `cargo deny` keeps an allow-list. GPL-2.0-only is not allowed, because it's incompatible with Apache-2.0.
- Branding: game names are used only descriptively ("supports iRacing"). No game logos, and no game names in the product name.

### 12.2 Parsers from scratch (clean-room)

All game parsers are written **from scratch**. We don't build on other telemetry tools.

**Allowed sources**
- Official documentation and SDKs from the game studios:
  - iRacing SDK (`irsdk_defines.h` + docs)
  - EA's "F1 25 UDP Specification" (2025 + 2026 Season Pack)
  - Studio 397's `SharedMemoryInterface.hpp` / `InternalsPlugin.hpp` from the LMU install
  - LMU's own `/swagger-schema.json`
- Our own observation: raw recordings from the games (shm dumps, UDP captures, `.ibt` files), hex inspection, experiments.
- General public facts that aren't code (e.g. "LMU needs *Enable Plugins* ON"). These are recorded in RESEARCH.md with a source.

**Not allowed**
- Copying, translating or porting code from other telemetry projects. That includes TinyPedal, pyLMUSharedMemory, CrewChief, SimHub, kerb, pyirsdk, iracing crates, F1 parser crates, simshmbridge, etc.
- Depending on such crates or libraries.
- Copying official headers into the repo: EA's header is "All rights reserved", and the LMU/iRacing header terms are unclear or need checking. We write **our own Rust struct definitions from the spec**; a comment cites the spec version and section.

**Practice**
- Every parser module documents its source at the top (e.g. "EA F1 25 UDP spec v9.0, PacketCarTelemetry2Data").
- Struct layouts get compile-time size assertions against the sizes in the spec (e.g. F1 `PacketMotionData` = 1325 bytes, LMU struct sizes measured from the live mapping).
- Correctness is proven with our own recorded fixtures (§10). Official lap times from the game are the reference.

## 13. Risks

| Risk | Mitigation |
|---|---|
| Game updates change struct layouts (LMU especially, F1 yearly) | Version detection (LMU `gameVersion`, F1 `packetFormat`, iRacing self-describing). Raw recordings let us re-parse. Fail loudly with a clear UI message |
| LMU lap distance only at 5 Hz | Interpolation using elapsed time + speed, validated against official lap length. Alternatively arc length from `mPos` |
| iRacing live has no world position | Dead reckoning + per-track outline cache, or `.ibt` Lat/Lon when available |
| Cross-user comparison needs identical track/car keys | Canonical key mapping per adapter, with a mapping table for renamed tracks/cars |
| Anti-cheat concerns | Only *read* officially provided interfaces (shm/UDP/REST GET). Never inject into game processes. LMU REST: only a fixed list of GET endpoints (some GETs have side effects, docs/games/lmu.md) |
| License contamination | Clean-room parsers from official specs only (§12.2). No vendored official headers. `cargo deny` + `reuse lint` in CI |
| Gaps in official docs (e.g. LMU field semantics, iRacing sign conventions) | Find out by experiment with our own recordings and document the findings in `docs/` |
| WebKitGTK quirks on Linux (Tauri) | Keep the frontend standard and test on Linux in CI (Playwright/WebKit). Login runs in the system browser (§9.3), so WebAuthn doesn't depend on the webview |
| Other drivers' data leaking into storage (privacy) | Filter before the spool (§5.2) + automated privacy test on fixtures |
| Access-control bugs in sharing | Single central authorization function on the server (`sharing::lap_readable`), used by every endpoint that returns other users' data. Exhaustive test matrix (§10) |
| Lost 2FA device / passkey | Recovery codes, multiple passkeys, admin reset, CLI reset for the first admin |

## 14. Open questions

None at the moment. Decisions are recorded in §1; add new ones there.
