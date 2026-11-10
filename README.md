# Rundenzeit

Record, review and compare your sim racing lap times. **Your data stays yours.** Everything is stored locally, with optional sync to a server you host yourself.

> Status: early development (milestone M2). iRacing live recording (Windows), `.ibt` import, session/lap browser and lap comparison work. Sessions and laps can be exported to `.rzx` files (and CSV) and imported on another installation.

- Vision: [IDEA.md](IDEA.md)
- Research on game telemetry and existing tools: [docs/RESEARCH.md](docs/RESEARCH.md)
- Architecture and roadmap: [docs/PLAN.md](docs/PLAN.md)
- Contributing: [CONTRIBUTING.md](CONTRIBUTING.md)

## Planned game support

| Game | Capture |
|---|---|
| iRacing | Windows (first milestone) |
| F1 25 (incl. 2026 Season Pack) | Windows |
| Le Mans Ultimate | Windows |

The desktop client runs on Windows and Linux. On Linux it can view, compare, import and sync data.

## License

- Apps (desktop client, server, web UI): [AGPL-3.0-or-later](LICENSE)
- Library crates (`rz-model`, game parsers `rz-source-*`): MIT OR Apache-2.0

See [docs/PLAN.md §12](docs/PLAN.md) for details.
