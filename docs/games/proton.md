# Proton bridge – shared-memory games on Linux

Implementation: `crates/sy-bridge` (`sy-bridge.exe`, and the client the
desktop app uses).

> **Status:** works end to end with a simulated Le Mans Ultimate:
> natively (`--simulate-lmu`, `crates/sy-bridge/tests`) and in Wine 11
> (Nov 2026): `sy-bridge.exe` in a Wine prefix read a fake `LMU_Data`
> (mapping, Hold/Data events and the writer's lock, `examples/fake_lmu.rs`)
> and the native client recorded the laps with the exact times, also when
> it connected in the middle of a session. **Checked with the real game**
> (Nov 2026): a practice at Fuji (Cadillac V-Series.R, five timed laps)
> recorded on Bazzite through the bridge, started by the Steam launch
> option, has the game's lap and sector times, the car setups from the
> REST API, and telemetry at 100 Hz with no gap longer than 20 ms, steadier
> than the Windows recordings (golden test
> `fuji_practice_through_the_proton_bridge` in
> `crates/sy-capture/tests/lmu_recordings.rs`). Still open: see *To
> verify*.

Games such as Le Mans Ultimate publish their telemetry as named Windows
shared memory. Under Proton that memory exists only inside Wine: a native
Linux program can't open it. UDP games (F1 25) don't have this problem,
because Wine uses the normal Linux network.

`sy-bridge.exe` is a small Windows program that runs **inside the game's
Proton prefix**, next to the game. It reads the game with the same code as
the Windows app and sends the player's records over `127.0.0.1` to the
Stintory app on Linux, which spools, decodes and stores them exactly like
on Windows.

```text
game ─shared memory─► sy-bridge.exe (in the prefix) ─127.0.0.1:20790─► Stintory (Linux)
```

## Setting it up

1. In Stintory on Linux open *Settings → Proton bridge*. It shows a Steam
   launch option like

   ```text
   PROTON_REMOTE_DEBUG_CMD=/home/you/.local/share/de.stintory.desktop/proton/sy-bridge.exe %command%
   ```

   (Stintory copies `sy-bridge.exe` there at every start, so the path
   stays the same across updates.)
2. In Steam: right-click the game → *Properties → General → Launch
   Options*, paste it. Keep other options in front of `%command%`, e.g.
   `PROTON_REMOTE_DEBUG_CMD=… gamemoderun %command%`.
3. Le Mans Ultimate also needs *Settings → Gameplay → Enable Plugins* on
   (docs/games/lmu.md).

Start the game as usual. Stintory can be started before or after it; the
bridge waits for it. *Settings → Proton bridge* and *Settings → Le Mans
Ultimate* show the connection and what the game is doing; the sidebar
shows the recording.

Only one game at a time can use the bridge port. If you change the port in
Stintory, the launch option changes too (`--port`).

## How it starts and ends

Proton's launcher (`proton` script, checked in Proton 9.0) supports
`PROTON_REMOTE_DEBUG_CMD`, meant for a remote debugger: it splits the value
like a shell (`shlex`), starts it **with the same Wine and the same
prefix** as the game, just before the game, and kills it when the game
exits. That is exactly what the bridge needs: it shares the game's
wineserver, so the game's named mappings and events are visible to it, and
Steam sees the game end when the game ends.

Steam runs launch options through a shell (to verify, see below), so a path with spaces is quoted
twice (`PROTON_REMOTE_DEBUG_CMD="'/home/a b/sy-bridge.exe'" %command%`);
Stintory writes the option that way when needed. Paths with `'`, `"`, `$`,
`` ` `` or `\` can't be used.

Any other way of running `sy-bridge.exe` inside the running game's prefix
**with the same Proton version** works too. Without `--port` it listens on
`127.0.0.1:20790`.

## What it sends

- Nothing until Stintory connects, and nothing is stored by the bridge.
  Every connection starts the game readers afresh, so Stintory gets the
  records a reader sends first (iRacing's variable table, the session info)
  even when it connects in the middle of a session.
- The game's state about once a second (also the heartbeat), and while the
  player drives, the game's raw records: exactly what the Windows reader
  spools, **already filtered to the player's car** (docs/PLAN.md §5.2).
  When the bridge's idle rules (the same as the Windows app's) end a
  recording, it says so and Stintory ends the recording too.
- Only on `127.0.0.1`, and to one Stintory at a time; a second connection
  is turned away with a message.
- Protocol: length-prefixed frames, control messages as JSON, records as
  binary (`crates/sy-bridge/src/protocol.rs`). Bridge and app must speak
  the same protocol version; otherwise both say which `sy-bridge.exe` to
  use.

Games: Le Mans Ultimate, and iRacing should it run in Proton (not tested;
iRacing doesn't support Proton).

## Troubleshooting

- *"Waiting for a game started with the bridge"* while the game runs: the
  launch option isn't set (or another option replaced it). Start the game
  with `PROTON_LOG=1` in front of the launch option; Proton then writes
  `~/steam-<appid>.log`, which also contains the bridge's messages
  (`sy-bridge: … waiting for Stintory`).
- *"can't listen on 127.0.0.1:20790"* in that log: another program (or a
  second game with the bridge) uses the port. Choose another port in
  Stintory and update the launch option.
- *Le Mans Ultimate is running* with the *Enable Plugins* hint: as on
  Windows, the game shares no telemetry until the setting is on and the
  game was restarted.

## Trying it without a game

```sh
cargo run -p sy-bridge -- --simulate-lmu 3 --time-scale 10
```

runs the bridge natively on Linux with a simulated LMU session (an out-lap
and three laps of a 2 km oval, ten times faster than real time). A running
Stintory connects to it and records the session like a real one.

To try the real shared-memory reader in Wine, publish a fake `LMU_Data` in
a prefix and run the bridge in the same prefix; the example client prints
what arrives (or use Stintory itself):

```sh
cargo build --release -p sy-bridge --target x86_64-pc-windows-gnu --bin sy-bridge --example fake_lmu
export WINEPREFIX=/tmp/stintory-wine                     # any prefix
wine target/x86_64-pc-windows-gnu/release/sy-bridge.exe &
wine target/x86_64-pc-windows-gnu/release/examples/fake_lmu.exe 2 4 &   # 2 laps, 4× speed
cargo run --release -p sy-bridge --example client        # native
```

## Building

```sh
rustup target add x86_64-pc-windows-gnu
npm run build:bridge               # target/x86_64-pc-windows-gnu/release/sy-bridge.exe
npm run build:desktop:linux:docker   # Linux packages with sy-bridge.exe inside (needs Docker)
npm run build:desktop:linux:host     # the same on this system, for trying them out
```

**Packages to hand out are built in an Ubuntu 24.04 container**
(`docker/linux-packages.Dockerfile`, needs Docker; `DOCKER=podman` for
Podman). A Linux program needs the glibc it was built against or newer,
and the AppImage bundles the build system's libraries: built on a rolling
distribution (openSUSE Tumbleweed) the AppImage needed glibc 2.44 and
failed elsewhere with `libjpeg.so.8: cannot open shared object file`.
The base sets the oldest systems the packages run on: Ubuntu 24.04
(glibc 2.39, supported until 2029) covers Ubuntu 24.04+, Debian 13+,
Fedora 40+ and the rolling distributions, not Ubuntu 22.04 / Debian 12.
(22.04, glibc 2.35, would be the oldest possible base with WebKitGTK 4.1,
which Tauri 2 needs; 26.04 would require glibc 2.43.) The AppImage gets the
base's WebKit and its fixes only while the base is supported; its window
shows only Stintory's own pages. The container runs as your user; its build and caches go to
`target/linux-packages/`, the packages to
`target/linux-packages/target/release/bundle/`.

Both download the AppImage runtime once with curl and hand it to the
AppImage tool (`LDAI_RUNTIME_FILE`): the tool's own download from GitHub
can hang forever. Extra arguments go to `tauri build`, e.g.
`npm run build:desktop:linux:docker -- --bundles appimage`.

The host build needs the MinGW-w64 cross compiler (Debian/Ubuntu:
`gcc-mingw-w64-x86-64`; openSUSE: `mingw64-cross-gcc`). A development build
of the app finds the cross-built `sy-bridge.exe` in `target/` on its own;
`STINTORY_BRIDGE_EXE=/path/to/sy-bridge.exe` points it at another one.

## To verify with the real game

- That the bridge uses the Hold/Data events and the lock in the game's
  prefix (it found `LMU_Data` and the REST API on `127.0.0.1:6397`, and the
  data arrived at the full rate, but a recording can't tell events from
  the polling fallback).
- That the bridge is killed when the game exits and Steam shows the game
  as stopped.
- The delay between the game and the app (not recorded anywhere).
