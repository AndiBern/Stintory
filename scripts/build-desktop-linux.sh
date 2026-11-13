#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Stintory Contributors
# SPDX-License-Identifier: AGPL-3.0-or-later

# Builds the Linux packages (deb, rpm, AppImage) with sy-bridge.exe inside
# (docs/games/proton.md). Extra arguments go to `tauri build`, e.g.
# `--bundles appimage`.
#
# appimagetool downloads the AppImage runtime from GitHub on every run, and
# that download can hang forever. So the runtime is fetched once with curl,
# kept in target/appimage-runtime/ and passed to linuxdeploy's AppImage
# plugin (LDAI_RUNTIME_FILE → appimagetool --runtime-file). Delete the file
# to get a newer runtime.

set -eu
cd "$(dirname "$0")/.."

runtime="$PWD/target/appimage-runtime/runtime-x86_64"
url="https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64"

if [ ! -s "$runtime" ]; then
	echo "Downloading the AppImage runtime ($url)"
	mkdir -p "$(dirname "$runtime")"
	curl -fsSL --retry 3 --max-time 120 -o "$runtime.tmp" "$url"
	mv "$runtime.tmp" "$runtime"
fi

npm run build:bridge
LDAI_RUNTIME_FILE="$runtime" npx tauri build --config apps/desktop/tauri.bridge.conf.json "$@"
