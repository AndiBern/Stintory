#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Stintory Contributors
# SPDX-License-Identifier: AGPL-3.0-or-later

# Builds the Linux packages (deb, rpm, AppImage) with sy-bridge.exe inside
# (docs/games/proton.md) on this system. Extra arguments go to `tauri
# build`, e.g. `--bundles appimage`.
#
# The packages need this system's glibc or newer, and the AppImage bundles
# this system's libraries: for packages to hand out, use
# scripts/build-linux-packages.sh (npm run build:desktop:linux:docker), which runs
# this script in an Ubuntu 24.04 container.
#
# appimagetool downloads the AppImage runtime from GitHub on every run, and
# that download can hang forever. So the runtime is fetched once with curl,
# kept in target/appimage-runtime/ and passed to linuxdeploy's AppImage
# plugin (LDAI_RUNTIME_FILE → appimagetool --runtime-file). Delete the file
# to get a newer runtime.

set -eu
cd "$(dirname "$0")/.."

runtime="${CARGO_TARGET_DIR:-$PWD/target}/appimage-runtime/runtime-x86_64"
url="https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64"

if [ ! -s "$runtime" ]; then
	echo "Downloading the AppImage runtime ($url)"
	mkdir -p "$(dirname "$runtime")"
	curl -fsSL --retry 3 --max-time 120 -o "$runtime.tmp" "$url"
	mv "$runtime.tmp" "$runtime"
fi

npm run build:bridge
# ship sy-bridge.exe as a resource (from wherever CARGO_TARGET_DIR points)
bridge="${CARGO_TARGET_DIR:-$PWD/target}/x86_64-pc-windows-gnu/release/sy-bridge.exe"
config="{\"bundle\":{\"resources\":{\"$bridge\":\"sy-bridge.exe\"}}}"
LDAI_RUNTIME_FILE="$runtime" npx tauri build --config "$config" "$@"
