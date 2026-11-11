# Stintory

Record, review and compare your sim racing lap times. **Your data stays yours.** Everything is stored locally, with optional sync to a server you host yourself.

> Status: early development (milestone M6). iRacing live recording (Windows), F1 25 recording over UDP (Windows, Linux, consoles), Le Mans Ultimate recording from the game's shared memory (Windows; not yet verified with the real game), `.ibt` import, session/lap browser and lap comparison work. Sessions and laps can be exported to `.stintory` files (and CSV) and imported on another installation. A self-hosted server (Docker) with accounts (password, TOTP, passkeys) and the web UI syncs your sessions between your computers, and lets you share sessions and laps with other users, teams, or everyone on the server. Compare your laps with the ones others share with you, in the browser or the desktop app.

- Vision: [IDEA.md](IDEA.md)
- Research on game telemetry and existing tools: [docs/RESEARCH.md](docs/RESEARCH.md)
- Architecture and roadmap: [docs/PLAN.md](docs/PLAN.md)
- Running a server: [docs/SERVER.md](docs/SERVER.md)
- Contributing: [CONTRIBUTING.md](CONTRIBUTING.md)

## Game support

| Game | Capture | Notes |
|---|---|---|
| iRacing | Windows; `.ibt` import everywhere | [docs/games/iracing.md](docs/games/iracing.md) |
| F1 25 (incl. 2026 Season Pack) | UDP: Windows, Linux (Proton), from PlayStation/Xbox or another PC | [docs/games/f1.md](docs/games/f1.md) |
| Le Mans Ultimate | Windows (shared memory; turn on *Enable Plugins* in the game) | [docs/games/lmu.md](docs/games/lmu.md) |

The desktop client runs on Windows and Linux. On Linux it can view, compare, import and sync data, and record F1 25.

## License

- Apps (desktop client, server, web UI): [AGPL-3.0-or-later](LICENSE)
- Library crates (`sy-model`, game parsers `sy-source-*`): MIT OR Apache-2.0

See [docs/PLAN.md §12](docs/PLAN.md) for details.
