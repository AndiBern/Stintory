# Running a Rundenzeit server

The server keeps your sessions and laps, lets you log in from any browser,
and (soon) syncs with the desktop app and lets you share laps with friends
and teams. It is meant to be self-hosted: you run it, you own the data.

> Status: milestone M3a, part 1. Accounts (password, two-factor
> authentication), the web UI and uploading `.rzx` files work. Sync with the
> desktop app, passkeys and sharing follow.

## What you need

- A machine with Docker and Docker Compose.
- A domain and a reverse proxy with TLS (Caddy, Traefik, nginx …). Browsers
  only allow secure cookies and (later) passkeys over HTTPS.

## Start

```sh
git clone <repository> rundenzeit && cd rundenzeit
cp docker/.env.example docker/.env
# fill in docker/.env: RZ_PUBLIC_URL, RZ_SECRET_KEY, POSTGRES_PASSWORD,
# RZ_ADMIN_USERNAME / RZ_ADMIN_PASSWORD (at least 12 characters)
docker compose -f docker/compose.yml --env-file docker/.env up -d --build
```

The server listens on `127.0.0.1:8080`. Point your reverse proxy at it, for
example with Caddy:

```
laps.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

Open `https://laps.example.com` and log in with the admin account. The admin
account is only created when the database has no users yet; you can remove
`RZ_ADMIN_PASSWORD` from `docker/.env` afterwards.

## Configuration

All settings are environment variables (or command-line flags, see
`rz-server --help`).

| Variable | Default | Meaning |
|---|---|---|
| `RZ_DATABASE_URL` | – | Postgres connection URL |
| `RZ_PUBLIC_URL` | `http://localhost:8080` | URL users open; must match what the browser shows (login requests from other origins are rejected) |
| `RZ_SECRET_KEY` | – | 32 random bytes, base64 (`openssl rand -base64 32`). Encrypts two-factor secrets: **keep it**, changing it disables everyone's 2FA |
| `RZ_DATA_DIR` | `/data` (image) | Telemetry files |
| `RZ_LISTEN` | `0.0.0.0:8080` | Listen address |
| `RZ_TRUST_PROXY` | `false` (`true` in compose) | Take the client address from `X-Forwarded-For`. Only enable behind a proxy that sets it |
| `RZ_MAX_UPLOAD_MB` | `512` | Largest `.rzx` upload |
| `RZ_ADMIN_USERNAME`, `RZ_ADMIN_PASSWORD` | – | First admin, created if there are no users |
| `RUST_LOG` | `info` | Log level |

Settings that change at runtime are on the **Admin** page: who may create an
account (only admins, or anyone), requiring two-factor authentication for
everyone, and (later) sharing with everyone on the server.

## Accounts

- Admins create accounts on the Admin page. The new user gets a temporary
  password and must choose their own at the first login.
- Two-factor authentication: Account page → *Set up*, scan the QR code with an
  authenticator app, confirm a code, and keep the 10 recovery codes.
- Lost the password or the authenticator: an admin resets it on the Admin
  page. If no admin can log in anymore, use the command line:

  ```sh
  docker compose -f docker/compose.yml exec server rz-server admin reset-user <name> --admin
  ```

  It prints a temporary password, removes 2FA and ends all sessions.
- Failed logins are limited per account (10 per 15 minutes) and per address.
  Logins, failures, 2FA changes and admin actions are in the security log on
  the Admin page.

## Backups

Back up both volumes: the database (`pg_dump`) and the data directory with
the telemetry files.

```sh
docker compose -f docker/compose.yml exec postgres pg_dump -U rundenzeit rundenzeit > rundenzeit.sql
docker run --rm -v rundenzeit_data:/data -v "$PWD":/backup debian:trixie-slim \
    tar czf /backup/rundenzeit-data.tgz -C /data .
```

## Privacy note

The server admin can technically read all data on the server (as with any
self-hosted service). Only use a server whose admin you trust, or run your
own.

## Development

```sh
docker compose -f docker/compose.dev.yml up -d   # Postgres on 127.0.0.1:55432
export RZ_TEST_DATABASE_URL=postgres://rundenzeit:rundenzeit@127.0.0.1:55432/rundenzeit
cargo test -p rz-server                           # API tests, one schema each

npm run build -w web
RZ_DATABASE_URL=$RZ_TEST_DATABASE_URL RZ_SECRET_KEY=$(openssl rand -base64 32) \
RZ_ADMIN_USERNAME=admin RZ_ADMIN_PASSWORD='admin password 123' \
cargo run -p rz-server
```
