# Rundenzeit – Implementation Plan

See [RESEARCH.md](RESEARCH.md) for the telemetry interfaces and the competitor analysis this plan is based on.

## 1. Decisions so far

| Topic | Decision |
|---|---|
| Stack | **Rust** for capture, storage, analysis and server. **Tauri 2** desktop client with a **Svelte 5 + TypeScript** web frontend (SvelteKit, static SPA build) that is reused as the server's web UI |
| Capture platforms | iRacing, F1 25, LMU capture on **Windows only**. Future Proton games get a small in-prefix helper (`rz-bridge.exe`) |
| Linux client | Full viewer: browse, compare, import/export, sync. Plus `.ibt` file import |
| MVP game | **iRacing** |
| Server | Docker container: sync + sharing + **web UI** for browsing/comparing laps |
| Privacy | Client is fully offline by default. No analytics and no network calls except to the user-configured server |
| License | Open source. **AGPL-3.0-or-later** for apps (client, server, web UI), **MIT OR Apache-2.0** for reusable library crates (see §12) |
| Parsers | **Written from scratch** from official game specs/SDKs and our own recordings. No code from, and no dependency on, other telemetry tools (TinyPedal, CrewChief, SimHub, kerb, pyirsdk, …) |
| Recorded data | **Player's own car only.** Other cars' telemetry and other drivers' personal data (names, IDs, ratings) are never stored (§5.2) |
| Sharing | Users create **teams** and invite others. Sessions/laps can be shared with **individual users, teams and/or everyone on the server**. Per-user default plus per-session/lap override (§9.2) |
| Export format | `.rzx` bundles (§8) |
| Login | **Username + password**, **passkeys** (WebAuthn) and **TOTP** 2FA. OIDC later (§9.3) |
| Raw data | Raw game data is spooled to disk and **deleted once processed**. A debug setting keeps the raw files (§5.1) |

## 2. Goals and non-goals

**Goals (v1)**
- Automatically record every session/lap while driving, with no clicks needed
- Session and lap browser, lap view with traces over distance, **compare two laps** (delta trace, track map, synced cursor)
- Export selected sessions/laps to a file; import files (own or other people's)
- Optional sync to a self-hosted server; teams; share with users/teams/everyone, with a per-user default plus per-session/lap override
- Secure login: password, passkeys, TOTP
- Web UI on the server with the same analysis views

**Non-goals for v1** (candidates for later): live overlays/HUD, voice coaching, AI tips, setup sharing, video sync, consoles (F1 UDP could work almost for free; see M5), mobile app, OIDC login.

**Never:** recording other cars or other drivers' data.

## 3. Architecture

```
 Windows PC (driving)                                     Self-hosted server (Docker)
┌───────────────────────────────────────────────┐        ┌──────────────────────────────┐
│ Game ──shm/UDP──► Source adapter (rz-capture) │        │ rz-server (axum)             │
│                    │ normalized Frames         │  HTTPS │  REST API  /api/v1/...       │
│                    ▼                           │◄──────►│  auth, sharing, sync         │
│               Recorder / lap segmenter         │  sync  │  PostgreSQL (metadata)       │
│                    │ Laps (immutable)          │        │  blob store (FS / S3)        │
│                    ▼                           │        │  serves web UI (same TS app) │
│      Local store: SQLite + Parquet blobs       │        └──────────────────────────────┘
│                    ▲                           │                    ▲
│  Tauri backend (rz-analysis, rz-store)         │                    │ browser
│                    ▲ invoke()                  │             Any device (Linux …)
│  Web frontend (TS): sessions, lap view, compare│
└───────────────────────────────────────────────┘
 Linux PC: same client, without the Windows-only source adapters (viewer + import + sync)
 Proton (later): game ─shm─► rz-bridge.exe (in prefix) ─localhost TCP─► client
```

Key idea: **the frontend talks to one API shape** (`ApiClient` interface). In the desktop app it calls the Tauri backend (`invoke`). In the browser it calls the server over HTTP. Both backends use the same Rust crates (`rz-model`, `rz-analysis`), so resampling, delta computation, etc. exist exactly once.

### 3.1 Repository layout (monorepo)

```
Cargo.toml                 # workspace
crates/
  rz-model/                # domain types: Session, Lap, Channel, Units, IDs, (de)serialization
  rz-capture/              # Source trait, raw spool + crash recovery, player-only filter hooks, recorder/lap segmenter
  rz-source-iracing/       # shm reader (cfg windows) + .ibt parser (all platforms)
  rz-source-f1/            # UDP parser 2025/2026 formats (all platforms)
  rz-source-lmu/           # LMU_Data reader (cfg windows) + REST poller
  rz-store/                # local SQLite + Parquet blob store, export/import bundles
  rz-analysis/             # distance resampling, delta, track map, sectors, (later) corners
  rz-sync/                 # sync client (HTTP)
  rz-server/               # axum server binary
  rz-bridge/               # (later) Windows helper for Proton prefixes
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
| Local DB | SQLite via `sqlx` (or `rusqlite`) | Single file, no service |
| Telemetry blobs | **Parquet** (`arrow`/`parquet` crates, zstd compression), one file per lap | Columnar + compressed; users can open their data in pandas/DuckDB/Excel-ish tools ("it's my data") |
| Server | `axum` + `sqlx` + **PostgreSQL**, blobs on local volume (S3-compatible optional) | Standard, easy Docker deploy |
| Auth | `argon2` (password hashes), `webauthn-rs` (passkeys), `totp-rs` (TOTP), cookie sessions for the web UI, per-device API tokens for clients. Later `openidconnect` | Self-hosters often have no IdP. Passkeys + TOTP give strong security without one |
| Frontend | **Svelte 5** (runes) + TypeScript, **SvelteKit** with `adapter-static` (SPA, no SSR), Vite, **uPlot** for traces, Canvas for track map, shadcn-svelte/Bits UI for UI components, TanStack Table for lap tables. Tests: Vitest + Playwright | Little boilerplate, easy to learn, built-in reactivity/state. A static build runs unchanged in Tauri and served by axum. uPlot handles 100k+ points smoothly with cursor sync across charts |
| Packaging | Tauri bundler: MSI/NSIS (Windows), AppImage/deb/rpm (Linux). Server: multi-stage Docker image | |
| CI | Forgejo Actions on an Ubuntu runner (`.forgejo/workflows/ci.yml`): `cargo fmt`/`clippy`/`test`, frontend check/lint/tests/build, `reuse lint`, `cargo deny`, license-direction script. Windows builds and Windows-only tests run locally for now; CI compile-checks Windows-only crates against the Windows target from M1 | License checks keep the license split (§12) enforced |

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
- **Canonical keys** make laps comparable across users: `iracing:<TrackID>:<config>`, `iracing:<CarID>`; `f1_25:<trackId>`, `f1_25:<teamId>/<formula>`; `lmu:<track name normalized>:<layout>`, `lmu:<vehicle model>`.

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
| extras | | `iracing.*` e.g. shock defl, `dcTractionControl` | `f1.ers_*`, `f1.drs`, `f1.active_aero`, `f1.overtake` | `lmu.virtual_energy`, `lmu.tc_level`, `lmu.motor_map`, ride heights |

Storage: samples at the **native rate** (iRacing 60 Hz, F1 = configured rate, LMU ~100 Hz). Resampling happens only in analysis. Estimated size: 60 Hz × ~40 channels × 4 B ≈ 10 KB/s raw, so a 90 s lap ≈ 0.9 MB raw and **~150–300 KB as Parquet+zstd**. One hour ≈ 6–10 MB. Fine for years of local data.

## 5. Capture pipeline

```rust
trait Source {
    fn name(&self) -> GameId;
    fn probe(&mut self) -> bool;                  // game running / data available?
    fn next(&mut self, timeout: Duration) -> Result<Option<SourceEvent>>;
}
enum SourceEvent {
    SessionInfo(SessionMeta),       // track/car/session type/setup changes
    Frame(Frame),                   // one sample of canonical + extra channels for the player car
    LapBoundary { completed: LapSummary }, // when the game tells us officially
    Rewind { to_time: f64 },        // F1 flashback, iRacing reset/tow
    SessionEnd,
}
```

- **Supervisor** (in the Tauri backend, background thread): probes all enabled sources every ~2 s. When one becomes active, it runs the source loop until the game is gone. Tray icon shows status ("Recording iRacing – Spa – lap 7").

### 5.1 Raw spool (raw data kept until processed)

Pipeline: `game → reader → raw spool file (append) → parser → recorder → Parquet + SQLite`.

- Every raw sample the reader receives is first appended to a **spool file** `<data>/raw/<game>-<session-uuid>.rzraw`. The format is a small header + length-prefixed, timestamped records, compressed in zstd frames so a crash only loses the last frame.
  - iRacing: the var header table (once and on change), the player-filtered session YAML (on change) and one data buffer per tick.
  - F1: UDP datagrams (filtered, §5.2).
  - LMU: only the `generic` block, the player's `VehicleTelemetry`, `ScoringInfo` and the player's `VehicleScoring` per update. Not the whole ~MB `LMU_Data` map.
- The parser consumes the live stream as usual. A spool file's state becomes `processed` once the session has ended **and** every lap from it is committed (Parquet written + SQLite row). Then the file is **deleted**.
- **Crash recovery:** at startup, spool files in state `recording`/`processing` are replayed through the same parser + recorder (dedup by lap ID). After a crash or power loss, nothing is lost, not even the lap in progress.
- `failed` (parser error): the file is kept and the UI shows "1 recording could not be processed", with an option to retry after an update or delete it. This is very useful after game updates that change layouts.
- **Debug setting "Keep raw recordings"** (Settings → Advanced/Debug, off by default): processed spool files are kept instead of deleted. The UI shows folder size, an "open folder" button and a "delete all raw files" button. Raw files are **never synced and never exported** in `.rzx`. A user can attach them manually to a bug report.
- Size: iRacing ≈ 60 ticks/s × ~1.5 KB ≈ 90 KB/s raw. zstd should bring that down to roughly 20–40 MB/h (to be measured). Only matters when "keep" is on.
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
- **iRacing specifics (M1):** wait on `IRSDKDataValidEvent`. Parse the var table once per session (re-parse if the header changes). Parse the session YAML on `sessionInfoUpdate` change. Player car only. `.ibt` import uses the same var-table parser plus the same recorder (file source instead of shm source).
- **F1 specifics (M5):** bind UDP port (configurable, default 20777). Group by `sessionUID`. Merge packets of the same `frameIdentifier` into one Frame (Motion + Telemetry + LapData + Status + MotionEx + Telemetry2). Branch on `packetFormat` 2025/2026. Player index from header. Optional UDP forwarding to other tools (many users run several F1 apps).
- **LMU specifics (M6):** open `LMU_Data`. Detect updates via `generic.events.SME_UPDATE_TELEMETRY` counter. Copy the struct and verify consistency (re-read if the counter changed during copy). Interpolate lap distance from 5 Hz scoring. Poll REST at ~1 Hz for virtual energy/setup/session info. Show a clear hint in the UI if the map is missing ("Enable Plugins in LMU Gameplay settings and restart").

## 6. Analysis (`rz-analysis`)

1. **Distance resampling**: resample each lap onto a fixed distance grid (e.g. every 1 m, or 0.5 m for short tracks) with linear interpolation (step for gear/booleans).
2. **Time delta**: Δt(d) = t_A(d) − t_B(d). Also the delta's derivative to highlight where time is gained/lost.
3. **Track map**: from `pos_x/y` (F1/LMU) or dead reckoning/GPS (iRacing). Store a smoothed outline per track (median of several clean laps) in `Track.outline`. Colour by delta/speed/throttle/brake.
4. **Sectors**: game sectors. Plus user-definable micro-sectors (later).
5. **Later – corner insights** (local, no AI service): detect corners from curvature/steering, then per corner compare braking point, min speed, throttle pick-up point, exit speed, and time gained/lost. Gives a "biggest gains" list like the paid tools.
6. **Theoretical best**: sum of best sectors (game sectors or micro-sectors).

Frontend gets resampled arrays (not raw) for comparison. These are computed on demand and cached in memory (LRU).

## 7. Client UI (desktop and server web UI share it)

1. **Dashboard / Recorder status**: which games are detected, current session, last laps.
2. **Sessions list**: filter by game/track/car/date/type; best lap; laps count; sharing badge; origin (own/imported/synced).
3. **Session detail**: lap table (time, sectors, valid, flags, fuel, tyres); select laps for comparison; share dialog; notes/tags.
4. **Lap view / Compare**: stacked traces vs distance (speed, throttle/brake, steering, gear, RPM, delta). Track map with synced cursor. Zoom/pan synced across charts. Choose "reference" = own best / any lap / imported / lap shared with me.
5. **Library of references**: laps from imports and laps shared with me (by users or teams) for the same track+car.
6. **Import/Export**: select sessions/laps → export bundle. Drag & drop `.rzx` / `.ibt` to import.
7. **Share dialog** (session or lap): "Use my default" or custom: an "Everyone on this server" toggle plus a picker for users and teams, with a summary of who can see it.
8. **Teams** (when connected to a server): my teams, create team, invite (by username or invite link), pending invites to accept/decline, members and roles, leave team.
9. **Account** (when connected): login via browser (§9.3), change password, manage passkeys, set up TOTP + recovery codes, list/revoke devices.
10. **Settings**: data directory, enabled games, F1 UDP port/forwarding, server URL, default sharing, auto-sync on/off. **Advanced/Debug:** "Keep raw recordings" + raw folder size/open/delete, log level.

## 8. Export / import format

`*.rzx` = ZIP:
```
manifest.json        format_version, exported_at, exporter version, driver display name (optional/anonymizable)
sessions/<id>.json   session metadata (incl. track/car keys)
laps/<id>.json       lap metadata
laps/<id>.parquet    telemetry (same file as in the local store; hash verified on import)
```
- Import dedups by lap ID/hash. Imported laps get `origin = imported` and keep the original driver name.
- Additional exports: **CSV** per lap (for spreadsheets), later **MoTeC `.ld`** (for i2 users).
- `format_version` is checked; newer minor versions are readable, unknown channels are ignored.

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
- Access check: user U may read a lap if U is the owner, or U is directly granted, or U is a member of a granted team, or there is an `everyone` grant (and it's allowed on the server). Team access is evaluated **live**: whoever joins later sees team-shared laps, and whoever leaves loses access.
- Changing the default affects everything still on `inherit`. The UI says so explicitly ("applies to 37 sessions using your default").
- Sharing grants read access to session/lap metadata and telemetry only. Others can never edit, re-share or delete.
- Revocation: clients that cached a foreign lap (used as a reference) get the lost-access list on their next sync and delete the cached copy. Copies already exported to `.rzx` can't be recalled; the share dialog mentions this.
- User lookup for the picker: exact username, or autocomplete among people you share a team with. There is no full user directory, so usernames on a server aren't enumerable.

### 9.3 Authentication

**Methods**
- **Username + password**: argon2id hashes, minimum length (NIST-style: length over complexity rules), login rate limiting per account + IP, generic error messages.
- **TOTP** (RFC 6238, 30 s, 6 digits) as a second factor. Setup with QR code + confirm code. 10 single-use **recovery codes** (stored hashed). Secrets are encrypted at rest with a server key from the environment.
- **Passkeys** (WebAuthn, `webauthn-rs`): usable for **passwordless login** (discoverable credentials) or as a second factor. A user can register several passkeys and remove them. Requires HTTPS and a stable origin: the RP ID = the server's domain from config (`localhost` works for testing).
- A user must keep at least one usable login method. Admin can enforce 2FA (TOTP or passkey) for all users.
- Account recovery: an admin can reset a user's password and 2FA. CLI command `rz-server admin reset-user <name>` for the very first admin.
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
GET  /tracks/{game}/{track}/{layout}/laps?car=…&shared_with_me=true   (best first)
POST /analysis/compare  {laps:[a,b], step}  → resampled channels + delta
# admin
GET/POST /admin/users  PATCH /admin/users/{id} (disable, reset password/2FA, admin flag)  GET/PUT /admin/settings (registration mode, enforce 2FA, allow_share_everyone)
```

### 9.5 Sync protocol (client side, `rz-sync`)
- Push: on lap completion (if auto-sync) or manually. Send manifest diff → upload missing blobs → upsert metadata. Retries with backoff and works offline-first (outbox table in SQLite).
- Pull: own data from other devices (e.g. view on Linux what was recorded on Windows), and **on-demand** fetch of laps shared with me when chosen as a reference (no full mirror). Teams and invites are refreshed on each sync. Cached foreign laps that lost access are deleted.
- Sharing changes made offline go into the outbox and apply on the next sync. The picker offers users/teams from the last cached list.
- Mutable fields (sharing, notes, tags): last-write-wins on `updated_at`.
- Deleting: tombstones synced both ways. Deleting on the server removes the blob when no other reference exists.

### 9.6 Deployment
- `docker-compose.yml`: `rundenzeit-server` + `postgres`. Volume for blobs. Env vars for public base URL (= WebAuthn origin/RP ID), secret key (TOTP encryption, cookies), admin bootstrap, registration mode (closed / invite-only / open).
- TLS via the user's reverse proxy (Caddy/Traefik/nginx). Documented, not built in. It is required for passkeys and secure cookies.
- Note in docs: the server admin can technically read all data on their server. That is fine for self-hosting, but we state it.

## 10. Testing & development strategy

Developing on Linux without the games running is a real constraint, so **record once, replay forever**:
- The **raw spool files** (§5.1) are the fixture format. Turn on "Keep raw recordings" on Windows, drive a few laps, and copy the `.rzraw` files.
- **Replay sources**: one per game, feeding `.rzraw` files through the exact same parser + recorder. Crash recovery uses the same code path. These are used in unit/integration tests and for UI development on Linux.
- Privacy test: a fixture recorded with other cars on track → stored data contains only the player (§5.2).
- Server tests: access-control matrix (owner / direct grant / team member / ex-member / everyone grant with the server setting on and off / stranger × inherit/custom at session and lap level), invite flows, auth flows (password, TOTP, recovery code, passkey via a software authenticator in tests).
- Fixtures in `fixtures/` (small, a few laps each). Larger ones via Git LFS or a download script.
- `.ibt` sample files also serve as iRacing fixtures.
- Golden tests: fixture → expected lap count, lap times (must match the game's official times within 1 ms), validity flags.
- Frontend: component tests + Playwright smoke test against the server with seeded data.

## 11. Roadmap

| Milestone | Content | Done when |
|---|---|---|
| **M0 – Skeleton** | Cargo workspace, Tauri app shell, `web/` with ApiClient abstraction, CI for Windows+Linux, `rz-model` + Parquet read/write, SQLite schema + migrations, license setup (per-crate `license` fields, SPDX headers, `CONTRIBUTING.md` with DCO, `deny.toml`) | CI green incl. license checks; empty app starts on Windows and Linux |
| **M1 – iRacing MVP** | iRacing shm source, raw spool + crash recovery + "keep raw" debug setting, player-only filtering, lap segmenter, local store, `.ibt` import (also on Linux), sessions list, session detail, lap view, compare 2 laps with delta + track map | Drive 10 laps → all laps appear with correct times. Kill the client mid-lap → after restart nothing is lost. Spool files are gone after processing (kept with debug setting). Compare best vs. second best with delta + map. Same data visible on Linux via `.ibt` import |
| **M2 – Export/Import** | `.rzx` bundles, CSV export, import with dedupe, "imported" origin in UI | Export on Windows → import on Linux → identical comparison |
| **M3a – Server, auth, own sync** | axum server, Postgres, Docker image + compose, admin bootstrap, password login, TOTP + recovery codes, passkeys, web sessions, browser-based device login + device tokens (keychain), sync push/pull of own data | Log in with each method. Record on Windows → appears in the Linux client via sync. Revoking a device token stops its sync |
| **M3b – Teams & sharing** | Teams, roles, invites (username + link), share grants with inherit/custom, default sharing, share dialog + teams UI in client, revocation of cached foreign laps | Access-control test matrix green. User B sees only laps shared with B or B's teams. Leaving a team removes access |
| **M4 – Web UI** | Serve `web/` from server with HttpApi; account/teams/sharing pages; "shared with me" browser per track/car | Compare own lap vs a team mate's shared lap in the browser |
| **M5 – F1 25** | UDP source (2025 + 2026 formats), flashback handling, UDP forwarding, console note in docs | Time trial + race laps recorded with correct validity. Flashback laps flagged |
| **M6 – LMU** | `LMU_Data` source, scoring interpolation, REST poller (VE, setup), setup hint UI | Laps recorded with correct times; VE/TC/ABS traces visible |
| **M7 – Proton bridge + more games** | `rz-bridge.exe` (localhost TCP), Steam launch-option docs; next games by demand (ACC, AC EVO, AMS2, rF2, Forza…) | A Proton game captured on Linux end-to-end |
| **Later** | OIDC login, corner insights, micro-sectors, theoretical best, MoTeC export, setups, live overlay | – |

M1 is the biggest milestone. A useful internal split is M1a (capture + store + session list) and M1b (lap view + compare).

## 12. Licensing & clean-room parsers

### 12.1 License split

| Component | License | Reason |
|---|---|---|
| `rz-server`, `apps/desktop`, `web/`, `rz-capture`, `rz-store`, `rz-sync`, `rz-analysis`, `rz-bridge` | **AGPL-3.0-or-later** | Anyone who changes these and distributes them, or runs them as a network service, must publish their changes. Nobody can turn Rundenzeit into a closed hosted service |
| `rz-model` (incl. `.rzx` format), `rz-source-iracing`, `rz-source-f1`, `rz-source-lmu` | **MIT OR Apache-2.0** | Other tools can use the parsers and read/write the export format. That helps data portability and the format becomes a de-facto standard |

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
| Anti-cheat concerns | Only *read* officially provided interfaces (shm/UDP/REST GET). Never inject into game processes. LMU REST: GET only |
| License contamination | Clean-room parsers from official specs only (§12.2). No vendored official headers. `cargo deny` + `reuse lint` in CI |
| Gaps in official docs (e.g. LMU field semantics, iRacing sign conventions) | Find out by experiment with our own recordings and document the findings in `docs/` |
| WebKitGTK quirks on Linux (Tauri) | Keep the frontend standard and test on Linux in CI (Playwright/WebKit). Login runs in the system browser (§9.3), so WebAuthn doesn't depend on the webview |
| Other drivers' data leaking into storage (privacy) | Filter before the spool (§5.2) + automated privacy test on fixtures |
| Access-control bugs in sharing | Single central authorization function on the server, used by every data endpoint. Exhaustive test matrix (§10) |
| Lost 2FA device / passkey | Recovery codes, multiple passkeys, admin reset, CLI reset for the first admin |

## 14. Open questions

None at the moment. All decisions are in §1.
