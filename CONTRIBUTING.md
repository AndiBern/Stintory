# Contributing to Stintory

Thanks for your interest! Please read this before opening a pull request.

## Licensing

Stintory uses two licenses (see [docs/PLAN.md §12](docs/PLAN.md)):

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

## Developer Certificate of Origin (DCO)

We use the [Developer Certificate of Origin](https://developercertificate.org/) instead of a CLA. Sign off every commit:

```sh
git commit -s -m "Your message"
```

This adds a `Signed-off-by: Your Name <you@example.com>` line, which certifies that you wrote the change or have the right to submit it under the project's licenses.

## Clean-room rule for game parsers

All game telemetry parsers are written **from scratch** (see [docs/PLAN.md §12.2](docs/PLAN.md)):

- Allowed sources: official game documentation/SDKs, and your own recordings and experiments.
- **Not allowed:** copying, translating or porting code from other telemetry tools (TinyPedal, CrewChief, SimHub, kerb, pyirsdk, existing F1/iRacing parser crates, …), or depending on them.
- Don't copy official SDK headers into the repo. Write Rust definitions from the spec and cite the spec version in a comment.

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
```

## Pull requests

- Keep PRs focused. Add tests for new behaviour.
- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and the frontend lint must pass.
