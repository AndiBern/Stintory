# SPDX-FileCopyrightText: 2026 The Stintory Contributors
# SPDX-License-Identifier: AGPL-3.0-or-later

# sy-server with the web UI. Build from the repository root:
#   docker build -f docker/Dockerfile -t stintory-server .

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
    cargo build --release --locked -p sy-server \
    && cp target/release/sy-server /sy-server

# --- runtime --------------------------------------------------------------
FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /data stintory \
    && mkdir -p /data && chown stintory /data
COPY --from=server /sy-server /usr/local/bin/sy-server
COPY --from=web /src/web/build /app/web
ENV STINTORY_WEB_DIR=/app/web \
    STINTORY_DATA_DIR=/data \
    STINTORY_LISTEN=0.0.0.0:8080
USER stintory
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["sy-server"]
CMD ["serve"]
