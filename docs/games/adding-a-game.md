# Adding a game

What it takes to support a new game, from the first raw recording to the settings page. The existing games are the examples: `sy-source-lmu` (shared memory + REST, Proton bridge), `sy-source-f1` (UDP) and `sy-source-iracing` (shared memory + `.ibt` files).

Read [CONTRIBUTING.md](../../CONTRIBUTING.md) first. Two of its rules decide whether a game can be added at all:

- **Clean room** ([ARCHITECTURE.md §12.2](../ARCHITECTURE.md#122-parsers-from-scratch-clean-room)): the parser is written from the studio's own documentation or SDK and your own recordings. Code from other telemetry tools, or depending on it, is not allowed. Without an official, documented interface (shared memory, UDP, REST, telemetry files) there is nothing to build on.
- **Player only** ([§5.2](../ARCHITECTURE.md#52-player-car-only)): other cars' telemetry and other drivers' names, ids and ratings are removed **before anything reaches disk**, including the raw spool.

Only *read* official interfaces. Never inject into the game process (anti-cheat, [§13](../ARCHITECTURE.md#13-risks)).

Open an issue (or comment in [Supported Games and Platforms](https://github.com/AndiBern/Stintory/discussions/2)) before starting, so work isn't done twice.

## Overview

| Step | Where | License |
|---|---|---|
| 1. Game id | `crates/sy-model/src/game.rs`, `web/src/lib/api/types.ts`, `web/src/lib/format.ts` | MIT OR Apache-2.0 / AGPL |
| 2. Adapter crate: raw source + decoder | `crates/sy-source-<game>/` (new) | MIT OR Apache-2.0 |
| 3. Pipeline | `crates/sy-capture/src/pipeline.rs` | AGPL |
| 4. Tests: synthetic, privacy, real recordings | `crates/sy-capture/tests/` | AGPL |
| 5. Desktop recording + settings | `apps/desktop/src/`, `crates/sy-store/src/settings.rs`, `crates/sy-model/src/api.rs`, `web/` | AGPL / MIT OR Apache-2.0 |
| 6. Proton bridge (shared-memory games only) | `crates/sy-bridge/` | AGPL |
| 7. Analysis and charts (optional) | `crates/sy-analysis/`, `web/src/lib/` | AGPL |
| 8. Docs | `docs/`, `README.md` | |

Adding the `GameId` variant first is a good start: the compiler then lists every Rust `match` that has to handle the new game (pipeline, conditions).

## 1. Game id

- Add a variant to `GameId` in `crates/sy-model/src/game.rs`, with `as_str`, `display_name` and an entry in `GameId::ALL`.
- **The string form is permanent.** It is stored in the SQLite and Postgres databases, in spool file headers, in `.stintory` bundles and in track/car keys. Pick it once (`f1_25`, `lmu`, …) and never change it.
- Mirror it in the frontend: the `GameId` union in `web/src/lib/api/types.ts` and the display name in `web/src/lib/format.ts`.

Older app versions don't know the new id. Sync is no problem (the app and the server must run the same version, [§9.5](../ARCHITECTURE.md#95-sync-protocol-client-side-sy-sync)), but a `.stintory` file with the new game can't be imported into an older app.

## 2. The adapter crate `sy-source-<game>`

A new crate under `crates/` (the workspace picks up `crates/*`). Add it to `[workspace.dependencies]` in the root `Cargo.toml`.

- `license = "MIT OR Apache-2.0"`, SPDX headers on every file. It may depend on `sy-model` (with `default-features = false`) and permissive third-party crates, **never on an AGPL crate** (CI checks this with `scripts/check-license-direction.mjs` and `cargo deny`).
- The crate docs (`lib.rs`) name the source: the official spec or SDK and its version ("written from scratch from …"). Don't copy official headers into the repository; write Rust definitions from the spec and cite the version and section in a comment.
- Live readers that only work on Windows go behind `#[cfg(windows)]`, so the crate still builds and tests everywhere.

The crate implements the two traits from `sy_model::capture` ([§5](../ARCHITECTURE.md#5-capture-pipeline)):

### `RawSource`: read the game

- Returns `RawRecord { kind, payload }`. The record kinds and the payload format are the adapter's own (`KIND_*` constants). Keep them stable: spool files are replayed later (crash recovery, "Keep raw recordings", bug reports), possibly by a newer version.
- **Filter to the player before returning a record.** Copy only the player's structs (LMU), zero other cars' array entries (iRacing, F1), drop per-car packets of other cars, reduce driver lists to the player. Session-level facts (track, weather, field size, own position) may stay.
- Record what the decoder needs and nothing more. Records are spooled to disk as they are ([§5.1](../ARCHITECTURE.md#51-raw-spool-raw-data-kept-until-processed)).
- Live sources return `Ok(None)` when nothing arrived within the timeout; file sources return `Ok(None)` at the end of the file.
- Shared-memory games: also provide a cheap probe (not running / running without telemetry / driving), as `sy_source_lmu::Probe` does. The desktop supervisor and the Proton bridge use it.

### `Decoder`: raw records → `SourceEvent`s

- `SourceEvent::Session(SessionMeta)` when a session starts or its info changes. A different `session_key` means a new session.
  - The **session key must contain the player's identity** (account id or name) as well as the game's session id. Session and lap ids are derived from it, and they must stay unique when two drivers of the same online session exchange laps (see the iRacing key in [iracing.md](iracing.md)).
  - **Canonical track and car keys** `<game>:…` make laps comparable across users. Use the game's stable ids where it has them (`iracing:<TrackID>:<config>`), otherwise normalized names plus something that tells layouts apart (`lmu:<track>:<layout length m>`). Add yours to [§4.1](../ARCHITECTURE.md#41-entities).
  - `channels` declares the channel list of every `Frame`. The recorder adds `t` and `dist` itself.
  - `session_info` is filtered to the player, like the raw records.
- `SourceEvent::Frame` per sample: session time, the game's lap number, lap distance, pit flag and the values in the declared order.
  - Map game data to the **canonical channels in SI units** (`sy_model::channel::names`, table in [§4.2](../ARCHITECTURE.md#42-channels-per-lap-columnar)): m/s, rad, m/s², °C, kPa, 0–1 for pedals. Check sign conventions against recordings (steering + = right, `g_lat` same sign).
  - Useful game-specific values go under a namespace: `<game>.<name>` (e.g. `lmu.virtual_energy`).
  - If the game sends lap distance at a low rate, interpolate it (LMU scoring is 5 Hz).
- `LapTime` with the game's **official** lap and sector times. The official time wins over the measured one.
- `Invalidate`, `Rewind` (flashbacks), `LineCrossed`, `LapAborted` (tow, reset, left the car) and `SessionEnd` where the game has them. Document your validity rules if the game has no "lap invalid" flag (iRacing).
- `finish()` returns the final events when the input ends.

Also add a `sim` module that produces synthetic records in the same layout as the live source (a car going round a simple track). It drives the tests below and lets others try the pipeline without the game. It is not a recording of the real game.

Struct layouts get compile-time size checks against the spec (`const _: () = assert!(size_of::<…>() == N);`).

## 3. Pipeline

In `crates/sy-capture/src/pipeline.rs`:

- `decoder_for`: return the new decoder.
- `recorder_config`: game-specific recorder settings, e.g. how long to wait for an official time that arrives late (iRacing), how long to hold laps back for flashbacks (F1), the minimum samples of a real lap (LMU).

Add the crate to `crates/sy-capture/Cargo.toml`. From here, live recording, crash recovery and import all work the same way, because they share this pipeline.

## 4. Tests

Development happens mostly without the game running: **record once, replay forever** ([§10](../ARCHITECTURE.md#10-testing--development-strategy)).

- **Synthetic tests** in `crates/sy-capture/tests/<game>.rs` with the `sim` records: laps, official times, validity, out/in laps, aborted laps, restarts, a new session. `tests/lmu.rs` is a good model.
- **Privacy test**: a session with other cars must produce stored data (and a spool) without other drivers' names or telemetry (`spool_replay_is_identical_and_player_only` in `tests/lmu.rs`).
- **Real recordings** in `crates/sy-capture/tests/<game>_recordings.rs`. Turn on *Settings → Advanced → Keep raw recordings*, drive, and put the `.syraw` files into `telemetry_examples/<GAME>/`. That folder is ignored by git: recordings are personal data and stay out of the repository. The tests skip without them (see `tests/lmu_recordings.rs`). Assert lap counts, validity and that lap times match the game's official times within 1 ms.
- Test sessions the game handles differently: practice, qualifying, race with a standing start, pit stops, resets/restarts, pausing, leaving to the menu, track limits.
- Windows-only code can't run in CI, but it must compile: add the crate to the `cargo clippy --target x86_64-pc-windows-msvc` line in `.github/workflows/ci.yml`. Test live reading on Windows yourself.

## 5. Desktop app

How the game is recorded depends on the interface. Copy the nearest existing game.

- **Supervisor** in `apps/desktop/src/recording.rs`: a thread that probes the game (every ~2 s), runs `ctx.record(GameId::…, &mut source, &mut idle_done)` while it delivers data, and decides when a recording ends (idle time; pauses may need a longer limit). `supervise_lmu` (shared memory), `supervise_f1` (UDP, port settings) and `supervise_iracing` (no settings) are the models. Start it in `spawn_supervisor` in the same file, with `#[cfg(windows)]` if it only works there.
- **Settings** (if the game needs any: on/off, ports, …):
  - a struct `<Game>CaptureSettings` and the status fields in `Settings` in `crates/sy-model/src/api.rs`,
  - a key in `crates/sy-store/src/settings.rs` (`capture.<game>`),
  - a shared state struct (`LmuCapture`) in `recording.rs` and `state.rs`, a `set_<game>_capture` command in `commands.rs`, registered in `lib.rs`, and the status in `get_settings`,
  - the frontend: types in `web/src/lib/api/types.ts`, the method in `client.ts`, `tauri.ts` and `http.ts` (the server web UI does not record), and a component like `web/src/lib/components/LmuCapture.svelte` on the settings page.
- **Hints**: when the game runs but shares no telemetry, tell the user what to change in the game (`LMU_PLUGINS_HINT`).
- **File import** (if the game writes telemetry files, like iRacing's `.ibt`): a file `RawSource` in the adapter crate, the extension in `import_files` in `commands.rs`, and the file dialog filters in `web/src/lib/api/tauri.ts`.

## 6. Proton bridge (shared-memory games)

Shared memory of a game in Proton is only visible inside its Wine prefix, so Linux needs `sy-bridge.exe` ([proton.md](proton.md)). UDP games need nothing here.

- Implement the `Game` trait (`crates/sy-bridge/src/server.rs`) in `crates/sy-bridge/src/windows.rs`, using the adapter's live source and probe. Keep its idle rules the same as the desktop supervisor's.
- Add the game to the list the app asks the bridge for (`supervise_bridge` in `apps/desktop/src/recording.rs`).
- Check it with the real game in Proton and note what is still open in proton.md.

## 7. Analysis and charts (optional)

Canonical channels work everywhere without changes. Game-specific extras need wiring:

- `crates/sy-analysis/src/lib.rs`: the list of channels the compare view gets, if an extra should be compared.
- `crates/sy-analysis/src/conditions.rs`: the channels for tyre temperature, wear, compounds, damage and weather, and compound names from the session info. The compiler asks for these once the `GameId` exists.
- `web/src/lib/charts/compare-panels.ts`: chart panels for extras. `web/src/lib/lap-conditions.ts`: the game's tyre compound colours.

## 8. Docs

- **`docs/games/<game>.md`**: how to set up the game, what is read and recorded, sessions, laps and validity, axes and signs, the session key, and a status line naming the recordings it was checked with. List what is not verified yet under *To verify with recordings*. Write down what you found out by experiment; that is the knowledge the next game update will need. [lmu.md](lmu.md) and [f1.md](f1.md) show the structure.
- `docs/RESEARCH.md` §1: the game's telemetry interface, with sources.
- `docs/ARCHITECTURE.md`: the canonical keys (§4.1), a column in the channel table (§4.2), what is spooled (§5.1), how the player is filtered (§5.2), the game's specifics (§5), allowed sources (§12.2), the license table (§12.1), §1 and §11 status. Keep the section numbers stable.
- `README.md`: a row in the supported games table.
- The MCP instructions in `crates/sy-mcp/src/lib.rs` name the supported games.

Game names are only used descriptively ("supports …"): no game logos ([§12.1](../ARCHITECTURE.md#121-license-split)).

## Checklist for the pull request

- [ ] Parser written from official docs and own recordings only; sources named in the crate docs
- [ ] Other cars and drivers removed before the spool; privacy test passes
- [ ] Session key contains the player's identity; canonical track/car keys documented
- [ ] Canonical channels in SI units, signs checked against recordings
- [ ] Synthetic tests, and real-recording tests that match the game's official lap times
- [ ] Windows-only code compile-checked in CI
- [ ] `docs/games/<game>.md`, README and ARCHITECTURE updated
- [ ] `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` and the frontend checks pass
