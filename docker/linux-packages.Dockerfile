# SPDX-FileCopyrightText: 2026 The Stintory Contributors
# SPDX-License-Identifier: AGPL-3.0-or-later

# Build environment for the Linux packages of the desktop app (deb, rpm,
# AppImage). Used by scripts/build-linux-packages.sh (npm run
# build:desktop:linux:docker); it needs no build context.
#
# Linux binaries run on the glibc they were built against and newer, and
# the AppImage bundles the build system's libraries. So the packages are
# built on an older LTS base: Ubuntu 24.04 (glibc 2.39, supported until
# 2029) runs them on Ubuntu 24.04+, Debian 13+, Fedora 40+ and the rolling
# distributions, not on Ubuntu 22.04 / Debian 12. Choose the newest base
# only when the oldest supported systems should change (docs/games/proton.md).

FROM ubuntu:24.04

ARG RUST_VERSION=1.99
ARG NODE_VERSION=24.14.0

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl file git pkg-config xz-utils xdg-utils \
        libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev librsvg2-dev \
        libayatana-appindicator3-dev libssl-dev \
        gcc-mingw-w64-x86-64 \
    && rm -rf /var/lib/apt/lists/*

# Node.js (official build; runs on glibc 2.28+)
RUN curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" \
        | tar -xJ -C /opt \
    && ln -s "/opt/node-v${NODE_VERSION}-linux-x64" /opt/node

# Rust, readable by every user: the container runs as the host user, whose
# caches (CARGO_HOME, HOME) live in the mounted repository's target dir.
ENV RUSTUP_HOME=/opt/rustup \
    PATH=/opt/cargo/bin:/opt/node/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs \
        | CARGO_HOME=/opt/cargo sh -s -- -y --no-modify-path --profile minimal \
            --default-toolchain "${RUST_VERSION}" \
            --component rustfmt,clippy --target x86_64-pc-windows-gnu \
    && chmod -R a+rX /opt/rustup /opt/cargo

# linuxdeploy and its plugins are AppImages: no FUSE in a container
ENV APPIMAGE_EXTRACT_AND_RUN=1
