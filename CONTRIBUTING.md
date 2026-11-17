# Contributing to Stintory

Thanks for your interest! Please read this before opening a pull request.

## Licensing

Stintory uses two licenses (see [docs/ARCHITECTURE.md §12](docs/ARCHITECTURE.md#12-licensing--clean-room-parsers)):

| Part | License |
|---|---|
| Apps and app crates (`apps/`, `web/`, `sy-capture`, `sy-store`, `sy-analysis`, `sy-sync`, `sy-server`, …) | AGPL-3.0-or-later |
| Library crates (`sy-model`, `sy-source-*`) | MIT OR Apache-2.0 |

Your contribution to a file is licensed under that file's license. Every source file starts with an SPDX header, for example:

<!-- REUSE-IgnoreStart -->
```rust
// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
```
<!-- REUSE-IgnoreEnd -->

Library crates (`MIT OR Apache-2.0`) must never depend on AGPL crates. CI checks this.

There is no CLA and no commit sign-off. By opening a pull request you confirm that you wrote the change or have the right to submit it under the licenses above. The pull request template asks you to tick this.

## Clean-room rule for game parsers

All game telemetry parsers are written **from scratch** (see [docs/ARCHITECTURE.md §12.2](docs/ARCHITECTURE.md#122-parsers-from-scratch-clean-room)):

- Allowed sources: official game documentation/SDKs, and your own recordings and experiments.
- **Not allowed:** copying, translating or porting code from other telemetry tools (TinyPedal, CrewChief, SimHub, kerb, pyirsdk, existing F1/iRacing parser crates, …), or depending on them.
- Don't copy official SDK headers into the repo. Write Rust definitions from the spec and cite the spec version in a comment.

Want to add a game? [docs/games/adding-a-game.md](docs/games/adding-a-game.md) lists every step, from the adapter crate to tests, settings and docs.

## Privacy rules

- Only the player's own car is ever stored. Data about other cars and drivers must be filtered before it reaches disk.
- The client makes no network calls except to the server the user configured. No analytics.

## Development setup

Requirements:
- Rust (stable, via `rustup`; the version is pinned in `rust-toolchain.toml`)
- Node.js ≥ 24 + npm
- Linux: WebKitGTK 4.1 and GTK 3 development packages, see the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)
  - openSUSE: `sudo zypper install webkitgtk3-devel gtk3-devel libsoup-devel librsvg-devel libayatana-appindicator3-devel libopenssl-devel`
  - Debian/Ubuntu: `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev librsvg2-dev libayatana-appindicator3-dev libssl-dev`

Common commands:

```sh
npm ci                                  # install frontend + Tauri CLI
cargo test --workspace                  # Rust tests
npm run check -w web && npm run lint -w web && npm run test:unit -w web -- --run
npm run dev:desktop                     # run the desktop app with hot reload
npm run build:bridge                    # sy-bridge.exe for Proton (needs MinGW-w64, docs/games/proton.md)
npm run build:desktop:linux             # the Linux Flatpak (needs flatpak-builder, docs/games/proton.md)
```

The Flatpak build uses Flathub's shared modules as a git submodule
(`apps/desktop/flatpak/shared-modules`); the build script fetches it when
it's missing.

## Pull requests

- Keep PRs focused. Add tests for new behaviour.
- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and the frontend lint must pass.
