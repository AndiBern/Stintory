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

1. In Stintory on Linux open *Settings → Games → Proton bridge*. It shows a Steam
   launch option like

   ```text
   PROTON_REMOTE_DEBUG_CMD=/home/you/.var/app/de.stintory.Stintory/data/de.stintory.Stintory/proton/sy-bridge.exe %command%
   ```

   (Stintory copies `sy-bridge.exe` there at every start, so the path
   stays the same across updates.)
2. In Steam: right-click the game → *Properties → General → Launch
   Options*, paste it. Keep other options in front of `%command%`, e.g.
   `PROTON_REMOTE_DEBUG_CMD=… gamemoderun %command%`.
3. Le Mans Ultimate also needs *Settings → Gameplay → Enable Plugins* on
   (docs/games/lmu.md).

Start the game as usual. Stintory can be started before or after it; the
bridge waits for it. *Settings → Games* (*Proton bridge* and *Le Mans
Ultimate*) show the connection and what the game is doing; the sidebar
shows the recording.

Only one game at a time can use the bridge port. If you change the port in
Stintory, the launch option changes too (`--port`).

## How it starts and ends

Proton's launcher (`proton` script, the same in Proton 9.0, 10.0 and
bleeding-edge) supports `PROTON_REMOTE_DEBUG_CMD`, meant for a remote
debugger: it splits the value like a shell (`shlex`) and starts it **with
the same Wine and the same prefix** as the game, just before the game. That
is exactly what the bridge needs: it shares the game's wineserver, so the
game's named mappings and events are visible to it.

Proton also kills that program when the game exits, but only after its
helper `steam.exe` returned, and that never happened: on Bazzite (Nov 2026)
Steam showed LMU as running until `sy-bridge.exe` was killed by hand. The
reason (Proton 10.0 `steam_helper/steam.c`, Wine `server/process.c`):

- `steam.exe` starts the game, marks itself as a Wine *system* process
  (`ProcessWineMakeProcessSystem`) and waits on the event Wine hands back.
  The wineserver sets it 3 s after the last *normal* process ended (that's
  how games whose launcher exits early keep running in Steam).
- The bridge is a normal process, and so is the `start.exe /exec` through
  which Wine runs a program given by a Unix path; it waits for the bridge.
  Proton's kill hits that `start.exe`, not the bridge.

So `steam.exe` waited for the bridge, and the bridge for nothing. Now the
bridge, when it sees it was started by Proton (`PROTON_REMOTE_DEBUG_CMD` in
its environment), continues as a detached copy of itself, so `start.exe`
ends at once; once the game runs it marks itself as a system process as
well; and it ends a few seconds after `steam.exe` is gone
(`crates/sy-bridge/src/lifetime.rs`). Reproduced and checked in Wine 11 with
a stand-in for Proton's `steam.exe` (`examples/fake_steam.rs`): with the old
bridge `steam.exe` never ended, now it ends 5 s after the game and the
bridge 3 s later. Confirmed with the real game on Bazzite (Nov 2026):
after quitting LMU, Steam shows it as stopped. A recording in progress
then ends as with a closed connection: what arrived is kept.

Steam runs launch options through a shell, so a path with spaces is quoted
twice (`PROTON_REMOTE_DEBUG_CMD="'/home/a b/sy-bridge.exe'" %command%`);
Stintory writes the option that way when needed. Paths with `'`, `"`, `$`,
`` ` `` or `\` can't be used.

Any other way of running `sy-bridge.exe` inside the running game's prefix
**with the same Proton version** records too, but may keep Steam showing
the game as running until the bridge is stopped (only the launch option
lets it detach). Without `--port` it listens on `127.0.0.1:20790`.

## What it sends

- Nothing until Stintory connects, and nothing is stored by the bridge.
  Every connection starts the game readers afresh, so Stintory gets the
  records a reader sends first (iRacing's variable table, the session info)
  even when it connects in the middle of a session.
- The game's state about once a second (also the heartbeat), and while the
  player drives, the game's raw records: exactly what the Windows reader
  spools, **already filtered to the player's car** (docs/ARCHITECTURE.md §5.2).
  When the bridge's idle rules (the same as the Windows app's) end a
  recording, it says so and Stintory ends the recording too.
- Only on `127.0.0.1`, and to one Stintory at a time; a second connection
  is turned away with a message.
- Protocol: length-prefixed frames, control messages as JSON, records as
  binary (`crates/sy-bridge/src/protocol.rs`). Bridge and app must speak
  the same protocol version; otherwise both say which `sy-bridge.exe` to
  use.

## Other shared-memory bridges (Le Mans Ultimate)

Tools like [simshmbridge](https://github.com/Spacefreak18/simshmbridge)
(`lmushm` + `lmubridge.exe`) or
[DataLink](https://github.com/LukasLichten/Datalink) (a map `LMU_Data`)
take a different route: before the game starts they create the
file `/dev/shm/LMU_Data` and, inside the prefix, a mapping named `LMU_Data`
backed by that file. The game then writes into the file's pages, and native
Linux programs read them directly; nothing is copied. The file is named
after the game's mapping, so every such bridge puts it at the same path.
[simd](https://github.com/Spacefreak18/simapi/tree/master/simd) (part of
simapi) can take over simshmbridge's Linux side: it creates the file
itself, starts `lmubridge.exe` in the game's prefix when the game is
started with `SIMD_BRIDGE_EXE=/path/to/lmubridge.exe %command%`, and stops
it when the game exits.

Checked with simshmbridge and the real game (Nov 2026): recorded through
*Another shared-memory bridge*.

For users who run such a bridge anyway, *Settings → Games → Le Mans
Ultimate → Read the game through* can be switched to *Another shared-memory
bridge*. The app then reads `/dev/shm/LMU_Data` itself
(`crates/sy-source-lmu/src/live/posix.rs`), with the same code that turns
the memory into records on Windows (`live.rs`), and asks `sy-bridge` only
for iRacing. Differences to the Proton bridge:

- Only the memory reaches Linux, not the game's events and lock. The app
  polls every 2 ms and keeps a copy only when a second copy right after it
  is the same; the game writes about 100 times a second.
- The other bridge has to be in place before the game starts, otherwise the
  game uses its own mapping and the file stays empty. The app says so when
  the game runs (its REST API answers) but has never written into the file.
- The file stays when the game exits (simshmbridge keeps it until `lmushm`
  is stopped, simd until simd exits). The app only starts recording when
  the telemetry moves, and ends a recording once the data stops changing
  and the REST API no longer answers.
- The file must hold at least 324,824 bytes (simshmbridge makes it 505,000
  bytes; DataLink needs a map `{ "name": "LMU_Data", "size": 505000 }` in
  its config, it has no preset for LMU).
- In the Flatpak, `/dev/shm` is the host's (`--device=shm`).

Without the game and without another bridge,

```sh
cargo run -p sy-source-lmu --example fake_lmu_shm -- 2 4
```

writes `/dev/shm/LMU_Data` the way the game does through such a bridge
(an out-lap and two laps, four times faster than real time) and answers on
the REST port, so a running Stintory with this setting records it. Like
`lmushm`, it leaves the file behind (`rm /dev/shm/LMU_Data`).

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
npm run build:bridge          # target/x86_64-pc-windows-gnu/release/sy-bridge.exe
npm run build:desktop:linux   # the Flatpak with sy-bridge.exe inside
```

The Linux app is a **Flatpak** (`apps/desktop/flatpak/`) on the GNOME
runtime, which brings WebKitGTK and the other libraries, so the same file
runs on every distribution with Flatpak. `npm run build:desktop:linux`
(`scripts/build-flatpak.sh`) needs `flatpak-builder` (the distribution's
package or `flatpak install --user flathub org.flatpak.Builder`) and the
MinGW-w64 cross compiler (Debian/Ubuntu: `gcc-mingw-w64-x86-64`; openSUSE:
`mingw64-cross-gcc`): the SDK has no MinGW, so `sy-bridge.exe` is built on
the host first and copied in. The build installs the runtime and SDK for
the user when missing, downloads the Rust toolchain and the npm and cargo
packages while it runs, and writes
`target/flatpak/Stintory_<version>_x86_64.flatpak`. Extra arguments go to
`flatpak-builder`.

```sh
flatpak install --user target/flatpak/Stintory_*_x86_64.flatpak
flatpak run de.stintory.Stintory
```

In the Flatpak the data directory is
`~/.var/app/de.stintory.Stintory/data/de.stintory.Stintory/`, and the
launch option points at the `sy-bridge.exe` there. Steam installed as a
Flatpak (`com.valvesoftware.Steam`) can't see that folder;

The Flatpak has no access to the user's files: *Import…* opens any file
through the file dialog (the portal shares it with the app), but files
dragged onto the window arrive with their host path, which the sandbox
can't see, so dropping doesn't import. The app then says to use
*Import…*.

A development build of the app finds the cross-built `sy-bridge.exe` in
`target/` on its own; `STINTORY_BRIDGE_EXE=/path/to/sy-bridge.exe` points
it at another one.

## To verify with the real game

- That the bridge uses the Hold/Data events and the lock in the game's
  prefix (it found `LMU_Data` and the REST API on `127.0.0.1:6397`, and the
  data arrived at the full rate, but a recording can't tell events from
  the polling fallback).
- The delay between the game and the app (not recorded anywhere).
