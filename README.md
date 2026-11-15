# Stintory

Record, review and compare your sim racing lap times. **Your data stays yours.** Everything is stored locally, with optional sync to a server you host yourself.

Most lap analysis tools send your telemetry to the vendor's servers so you can compare yourself with everyone else. Stintory sends nothing to any company: it stores your laps on your computer, and if you want to share them with friends or a team, you do that through your own server.

> **Status:** the first version with all core features is done (v0.2.0). Stintory is now growing through new features and refinements of the existing ones. See [docs/ARCHITECTURE.md §11](docs/ARCHITECTURE.md#11-status) for what works and the known gaps.

## Features

- **Automatic recording** of every session and lap while you drive, no clicks needed. Only your own car is recorded, never other drivers' data.
- **Session and lap browser**, lap view with traces over distance, and **lap comparison** with a time delta, a track map with a synced cursor and **corner insights**: where you lose time, corner by corner (braking point, minimum speed, throttle pick-up, exit speed).
- **Export and import** sessions and laps as `.stintory` files (and CSV), to move them to another computer or compare with laps from other people.
- **Self-hosted server** (Docker) with accounts (password, TOTP, passkeys) and a web UI. It syncs your sessions between your computers and lets you share sessions and laps with other users, teams, or everyone on the server. Compare your laps with the ones others share with you, in the browser or the desktop app.
- **AI assistants** (Claude and other MCP clients) can read your laps and the corner analysis to tell you where you lose time: from the desktop app on your computer (*Settings → AI*) or from your server ([docs/SERVER.md](docs/SERVER.md)). Stintory has no AI service of its own.

## Game support

| Game | Capture | Notes |
|---|---|---|
| iRacing | Windows; `.ibt` import everywhere | [docs/games/iracing.md](docs/games/iracing.md) |
| F1 25 (incl. 2026 Season Pack) | UDP: Windows, Linux (Proton), from PlayStation/Xbox or another PC | [docs/games/f1.md](docs/games/f1.md) |
| Le Mans Ultimate | Windows (shared memory; turn on *Enable Plugins* in the game); Linux (Proton) through the Proton bridge | [docs/games/lmu.md](docs/games/lmu.md), [docs/games/proton.md](docs/games/proton.md) |

All three games are checked with real recordings. The desktop app runs on Windows and Linux. On Linux it does everything the Windows app does except recording iRacing live (iRacing doesn't support Proton; `.ibt` import works). It records F1 25, and Le Mans Ultimate running in Proton with the help of `sy-bridge.exe`, a small helper that runs next to the game ([docs/games/proton.md](docs/games/proton.md)). More games may follow by demand.

## Documentation

- Running a server: [docs/SERVER.md](docs/SERVER.md)
- Architecture, design rules and status: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- Per-game behaviour, verified against real recordings: [docs/games/](docs/games/)
- Game telemetry interfaces (incl. data not used yet), competitors and sources: [docs/RESEARCH.md](docs/RESEARCH.md)
- Building from source and contributing: [CONTRIBUTING.md](CONTRIBUTING.md)

## AI disclosure

Stintory is developed with the help of AI coding assistants, as is current state of the art in software development. Large parts of the code and documentation were written with them. Deciding what gets built, reviewing and testing the changes, and checking every game against real recordings before it counts as supported are done by me.

## License

- Apps (desktop client, server, web UI): [AGPL-3.0-or-later](LICENSE)
- Library crates (`sy-model`, game parsers `sy-source-*`): MIT OR Apache-2.0

See [docs/ARCHITECTURE.md §12](docs/ARCHITECTURE.md#12-licensing--clean-room-parsers) for details.
