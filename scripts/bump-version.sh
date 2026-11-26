#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Stintory Contributors
# SPDX-License-Identifier: AGPL-3.0-or-later

# Sets the version of Stintory: scripts/bump-version.sh 0.7.0
#
# Changes the workspace version and the internal crate dependencies in
# Cargo.toml, the workspace members in Cargo.lock, and the version of the
# web frontend in web/package.json and package-lock.json. Everything else
# (the desktop app, the Flatpak metainfo, the release check) reads the
# version from Cargo.toml. Commit the result and tag it as v<version>.
#
# Needs cargo and npm.

set -eu
cd "$(dirname "$0")/.."

if [ $# -ne 1 ]; then
	echo "usage: $0 <version>   (e.g. 0.7.0 or 0.7.0-beta.1)" >&2
	exit 2
fi
new=${1#v}

if ! printf '%s\n' "$new" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
	echo "error: '$new' isn't a version like 0.7.0 or 0.7.0-beta.1" >&2
	exit 2
fi

old=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1)
if [ -z "$old" ]; then
	echo "error: no version found in Cargo.toml" >&2
	exit 1
fi
if [ "$old" = "$new" ]; then
	echo "The version is already $new."
	exit 0
fi

# the escaped old version, so its dots match only dots
old_re=$(printf '%s' "$old" | sed 's/[.]/\\./g')

# [workspace.package] version and the internal crates (path = "crates/…")
sed -i \
	-e "s/^version *= *\"$old_re\"/version = \"$new\"/" \
	-e "/path = \"crates\//s/version = \"$old_re\"/version = \"$new\"/" \
	Cargo.toml

# the workspace members in Cargo.lock, without updating any dependency
cargo update --workspace --offline --quiet 2>/dev/null || cargo update --workspace --quiet

# web/package.json and its entry in package-lock.json
npm version "$new" --workspace web --no-git-tag-version --allow-same-version >/dev/null

left=$(git grep -n -F "\"$old\"" -- Cargo.toml web/package.json || true)
if [ -n "$left" ]; then
	echo "warning: the old version $old is still in:" >&2
	echo "$left" >&2
fi

echo "Version $old -> $new"
git status --short
