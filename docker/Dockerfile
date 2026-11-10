# SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
# SPDX-License-Identifier: AGPL-3.0-or-later

# rz-server with the web UI. Build from the repository root:
#   docker build -f docker/Dockerfile -t rundenzeit-server .

# --- web UI ---------------------------------------------------------------
FROM node:24-trixie-slim AS web
WORKDIR /src
COPY package.json package-lock.json ./
COPY web/package.json web/
# the desktop CLI (root devDependency) is not needed here
RUN npm ci --workspace web --include-workspace-root=false --ignore-scripts
COPY web/ web/
RUN npm run build -w web

# --- server ---------------------------------------------------------------
FROM rust:1.99-slim-trixie AS server
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p rz-server \
    && cp target/release/rz-server /rz-server

# --- runtime --------------------------------------------------------------
FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /data rundenzeit \
    && mkdir -p /data && chown rundenzeit /data
COPY --from=server /rz-server /usr/local/bin/rz-server
COPY --from=web /src/web/build /app/web
ENV RZ_WEB_DIR=/app/web \
    RZ_DATA_DIR=/data \
    RZ_LISTEN=0.0.0.0:8080
USER rundenzeit
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["rz-server"]
CMD ["serve"]
