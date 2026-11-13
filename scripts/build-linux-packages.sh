#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Stintory Contributors
# SPDX-License-Identifier: AGPL-3.0-or-later

# Builds the Linux packages (deb, rpm, AppImage) in an Ubuntu 24.04
# container (docker/linux-packages.Dockerfile), so they run on Ubuntu 24.04
# and every newer distribution. Packages built directly on a newer system
# need its glibc, and the AppImage would bundle its libraries.
#
# Runs scripts/build-desktop-linux.sh inside the container as the current
# user, with the repository mounted. Its build and caches go to
# target/linux-packages/ (the packages to target/linux-packages/target/
# release/bundle/), so the normal target/ isn't mixed with them. Extra
# arguments go to `tauri build`, e.g. `--bundles appimage`.
#
# Needs Docker (or Podman: DOCKER=podman). Uses the host's node_modules
# (npm ci first).

set -eu
cd "$(dirname "$0")/.."

docker=${DOCKER:-docker}
image=stintory-linux-packages
rust=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)

if [ ! -d node_modules ]; then
	echo "node_modules is missing; run npm ci first" >&2
	exit 1
fi

# no build context: the Dockerfile copies nothing (keeps docker/.env out)
"$docker" build --build-arg "RUST_VERSION=$rust" -t "$image" - < docker/linux-packages.Dockerfile

work=target/linux-packages
mkdir -p "$work/home" "$work/cargo"

# label=disable: the container may read the repository despite SELinux,
# without relabelling the user's files
"$docker" run --rm --init \
	--user "$(id -u):$(id -g)" \
	--security-opt label=disable \
	-v "$PWD:/src" -w /src \
	-e HOME="/src/$work/home" \
	-e CARGO_HOME="/src/$work/cargo" \
	-e CARGO_TARGET_DIR="/src/$work/target" \
	"$image" sh scripts/build-desktop-linux.sh "$@"

echo "Packages: $work/target/release/bundle/"
