#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Stintory Contributors
# SPDX-License-Identifier: AGPL-3.0-or-later

# Builds the Flatpak of the desktop app (apps/desktop/flatpak/) as a single
# file to hand out: target/flatpak/Stintory_<version>_x86_64.flatpak.
# Extra arguments go to flatpak-builder, e.g. --disable-rofiles-fuse.
#
# Needs flatpak-builder (the distribution's package or the
# org.flatpak.Builder Flatpak) and the MinGW-w64 cross compiler for
# sy-bridge.exe (docs/games/proton.md). The GNOME runtime and SDK are
# installed from Flathub for the user when missing. The build and its
# caches go to target/flatpak/.

set -eu
cd "$(dirname "$0")/.."

if command -v flatpak-builder >/dev/null 2>&1; then
	builder=flatpak-builder
elif flatpak info org.flatpak.Builder >/dev/null 2>&1; then
	builder="flatpak run org.flatpak.Builder"
else
	echo "flatpak-builder is missing: install it, or run" >&2
	echo "  flatpak install --user flathub org.flatpak.Builder" >&2
	exit 1
fi

manifest=apps/desktop/flatpak/io.github.andibern.stintory.yml
if [ ! -f apps/desktop/flatpak/shared-modules/README.md ]; then
	git submodule update --init apps/desktop/flatpak/shared-modules
fi

version=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1)
out=target/flatpak
mkdir -p "$out"

npm run build:bridge
# the manifest takes it from target/flatpak/, wherever CARGO_TARGET_DIR points
cp "${CARGO_TARGET_DIR:-target}/x86_64-pc-windows-gnu/release/sy-bridge.exe" "$out/"

flathub=https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak remote-add --user --if-not-exists flathub "$flathub"
$builder --user --install-deps-from=flathub --force-clean \
	--state-dir="$out/state" --repo="$out/repo" "$@" \
	"$out/build" "$manifest"
# --runtime-repo: installing the file adds Flathub for the runtime if needed
flatpak build-bundle --runtime-repo="$flathub" \
	"$out/repo" "$out/Stintory_${version}_x86_64.flatpak" io.github.andibern.stintory
echo "Flatpak: $out/Stintory_${version}_x86_64.flatpak"
