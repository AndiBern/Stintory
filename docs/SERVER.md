# Running a Rundenzeit server

The server keeps your sessions and laps, lets you log in from any browser,
and (soon) syncs with the desktop app and lets you share laps with friends
and teams. It is meant to be self-hosted: you run it, you own the data.

> Status: milestone M3a done. Accounts (password, TOTP, passkeys), the web UI,
> uploading `.rzx` files and sync with the desktop app work. Teams and
> sharing follow (M3b).

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
| `RZ_DB_WAIT_SECONDS` | `300` | How long to wait at startup for the database. Every failed attempt is logged with the reason (wrong password, unknown host …) |
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
- Passkeys: Account page → *Passkeys → Add passkey* (phone, Windows Hello,
  Touch ID, a password manager or a security key). Then *Log in with a
  passkey* works without username and password, and after a password login a
  passkey can be the second step. A passkey counts as two-factor
  authentication, also when an admin requires it.
  - Passkeys are bound to the server's address (`RZ_PUBLIC_URL`). Browsers
    only allow them over HTTPS (or on `localhost`). If you change the domain,
    everyone has to add their passkeys again.
- Lost the password or the authenticator: an admin resets it on the Admin
  page. If no admin can log in anymore, use the command line:

  ```sh
  docker compose -f docker/compose.yml exec server rz-server admin reset-user <name> --admin
  ```

  It prints a temporary password, removes 2FA (TOTP and passkeys) and ends
  all sessions.
- Failed logins are limited per account (10 per 15 minutes) and per address.
  Logins, failures, 2FA changes and admin actions are in the security log on
  the Admin page.

## Connecting the desktop app

In the app: **Settings → Server sync**, enter the server address and click
*Connect in browser…*. Your browser opens the server; log in there and click
*Allow*. The app then syncs automatically: shortly after each recorded lap,
after imports and deletions, and every few minutes.

- Everything you record on one computer appears on your other computers and
  in the web UI. Deleting a session deletes it everywhere.
- The app keeps its login (a device token) in the system keychain (Windows
  Credential Manager, Secret Service/KWallet on Linux). Without a keychain it
  is kept in the app's data folder; the settings page says so.
- Connected apps are listed on the **Account** page, where you can disconnect
  them. A disconnected app stops syncing and shows a notice.
- If the browser login can't reach the app (unusual setups), create a token on
  the Account page (*Desktop apps → Create a token by hand*) and paste it in the
  app (*Use a device token instead*).
- Headless machines: `cargo run -p rz-sync --example sync -- <data-dir> <server-url>`
  connects and syncs a data directory from the command line.

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
