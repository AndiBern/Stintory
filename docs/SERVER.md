# Running a Stintory server

The server keeps your sessions and laps, lets you log in from any browser,
syncs with the desktop app and lets you share laps with friends and teams.
It is meant to be self-hosted: you run it, you own the data.

> Status: all features work: accounts (password, TOTP, passkeys), the web
> UI, uploading `.stintory` files, sync with the desktop app, teams and
> sharing, comparing your laps with the ones others share with you, and an
> MCP endpoint for AI assistants.

## What you need

- A machine with Docker and Docker Compose.
- A domain and a reverse proxy with TLS (Caddy, Traefik, nginx …). Browsers
  only allow secure cookies and passkeys over HTTPS.

## Start

```sh
git clone <repository> stintory && cd stintory
cp docker/.env.example docker/.env
# fill in docker/.env: STINTORY_PUBLIC_URL, STINTORY_SECRET_KEY, POSTGRES_PASSWORD,
# STINTORY_ADMIN_USERNAME / STINTORY_ADMIN_PASSWORD (at least 12 characters)
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
`STINTORY_ADMIN_PASSWORD` from `docker/.env` afterwards.

## Configuration

All settings are environment variables (or command-line flags, see
`sy-server --help`).

| Variable | Default | Meaning |
|---|---|---|
| `STINTORY_DATABASE_URL` | – | Postgres connection URL |
| `STINTORY_PUBLIC_URL` | `http://localhost:8080` | URL users open; must match what the browser shows (login requests from other origins are rejected) |
| `STINTORY_SECRET_KEY` | – | 32 random bytes, base64 (`openssl rand -base64 32`). Encrypts two-factor secrets: **keep it**, changing it disables everyone's 2FA |
| `STINTORY_DATA_DIR` | `/data` (image) | Telemetry files |
| `STINTORY_LISTEN` | `0.0.0.0:8080` | Listen address |
| `STINTORY_TRUST_PROXY` | `false` (`true` in compose) | Take the client address from `X-Forwarded-For`. Only enable behind a proxy that sets it |
| `STINTORY_OAUTH_CLIENT_METADATA` | `true` | Fetch the client metadata documents of AI assistants that connect with OAuth (needs internet access). Set `false` on a server without internet: the assistants then register themselves (see AI assistants) |
| `STINTORY_MAX_UPLOAD_MB` | `512` | Largest `.stintory` upload |
| `STINTORY_DB_WAIT_SECONDS` | `300` | How long to wait at startup for the database. Every failed attempt is logged with the reason (wrong password, unknown host …) |
| `STINTORY_ADMIN_USERNAME`, `STINTORY_ADMIN_PASSWORD` | – | First admin, created if there are no users |
| `RUST_LOG` | `info` | Log level |

Settings that change at runtime are on the **Admin** page: who may create an
account (only admins, or anyone), requiring two-factor authentication for
everyone, and whether users may share with everyone on the server.

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
  - Passkeys are bound to the server's address (`STINTORY_PUBLIC_URL`). Browsers
    only allow them over HTTPS (or on `localhost`). If you change the domain,
    everyone has to add their passkeys again.
- Lost the password or the authenticator: an admin resets it on the Admin
  page. If no admin can log in anymore, use the command line:

  ```sh
  docker compose -f docker/compose.yml exec server sy-server admin reset-user <name> --admin
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
- Headless machines: `cargo run -p sy-sync --example sync -- <data-dir> <server-url>`
  connects and syncs a data directory from the command line.

## Teams and sharing

Nothing is shared until a user says so.

- **Teams** (page *Teams*): anyone can create a team and invite others, by
  username (they accept or decline under *Teams*) or with an invite link for
  people who have an account on the server. Owners and admins manage
  members; owners can also rename and delete the team. A team always keeps
  an owner.
- **Sharing a session or a lap**: *Share…* on the session page. Choose
  people (by exact username), teams you are in, and/or everyone on this
  server. A lap can be shared differently from its session, in both
  directions.
- **Default** (page *Sharing*): who sees what you record unless you choose
  something else. It applies to everything that still uses the default, and
  to new recordings. Imported sessions are never shared by default.
- **Shared with me** (page *Sharing*): sessions others share with you. Open
  one to look at the laps and compare them. *By track and car* lists every
  track and car with shared laps, the fastest one and how far your best lap
  is from it; open one to pick laps (yours and theirs) and compare them.
- **Comparing with others**: on a session page, select your lap and click
  *Compare with…*: it lists the fastest laps of the same track and car,
  yours (also imported ones) and those shared with you. The lap you choose
  becomes the reference; on the comparison page *Use as reference* changes
  it and *Add laps…* adds more.
- Others can only look: they can't change, delete or share your laps. They
  get the laps and the session's basic data (track, car, conditions), not
  your car setup.
- Access is checked on every request. Leaving or being removed from a team
  ends the access to what is shared with it, and ends what that person
  shared with the team. Deleting a team does the same for everyone.
- In the desktop app, *Sharing* and *Teams* appear once it is connected to
  the server. A shared session you open there is kept on the computer as a
  reference (also a lap you choose under *Compare with…*); the next sync
  deletes it when it isn't shared with you anymore,
  and disconnecting from the server deletes all such copies. Exported files
  can't be taken back.
- Admins can turn off "everyone on this server" on the Admin page (useful on
  larger servers). What was shared with everyone is then hidden from others
  until it is turned on again.
- There is no list of users: people find each other by exact username, or
  among the members of their teams.
- Team changes (created, joined, left, roles, removed, deleted) are in the
  security log.

## AI assistants (MCP)

An AI assistant can read your laps and the corner analysis and tell you
where you lose time, for example Claude Code or any other client of the
Model Context Protocol (MCP). The server offers an MCP endpoint at
`https://laps.example.com/mcp`.

**With OAuth** (claude.ai and Claude Desktop custom connectors, Claude
Code, other clients that support MCP authorization): add
`https://laps.example.com/mcp` as a connector or server. The assistant
opens your browser; log in and click *Allow*. In Claude Code:

```sh
claude mcp add --transport http stintory https://laps.example.com/mcp
```

then `/mcp` → *Authenticate*.

**With a token** (clients without OAuth): Account page → *AI assistants*
→ *Create token*, shown once, and give it to the assistant as an
`Authorization: Bearer` header:

```sh
claude mcp add --transport http stintory https://laps.example.com/mcp \
    --header "Authorization: Bearer syd_…"
```

Either way the assistant can only read: it can't change, delete or share
anything, and nothing but the MCP endpoint accepts its access. Ask, for
example: "Compare my last Snetterton laps with my best one.
   Where do I lose the most time, and what should I change?"

The assistant sees what you see in the web UI: your own sessions and laps
and the laps others share with you. Its tools: `list_tracks`,
`list_sessions`, `get_session`, `best_laps` (yours and shared, per track
and car), `compare_laps` (corner by corner: time lost, braking point, apex
speed, throttle, exit speed) and `lap_telemetry` (traces of a part of a
lap).

- Connected assistants (OAuth and tokens) are listed on the Account page
  under *AI assistants*; *Remove* ends the access at once. Tool calls are
  limited to 600 per 15 minutes per user.
- OAuth details: the server is its own authorization server (discovery at
  `/.well-known/oauth-protected-resource/mcp` and
  `/.well-known/oauth-authorization-server`). Clients identify themselves
  with a client ID metadata document (an https URL the server fetches;
  addresses in private networks are refused) or register dynamically.
  Authorization code with PKCE (S256), access tokens valid for an hour and
  only for `/mcp`, refresh tokens renewed on every use (90 days without
  use end the connection).
- Fetching client metadata documents needs internet access. On a server
  without it set `STINTORY_OAUTH_CLIENT_METADATA=false`: assistants then
  register themselves. If an assistant already tried with its metadata
  document, remove its saved login for the server (Claude Code: `/mcp` →
  the server → *Clear authentication*, or add the server again) so it
  registers on the next attempt.
- `STINTORY_PUBLIC_URL` must be the address the assistant uses: the OAuth
  issuer and the token's audience are derived from it.
- Protocol: Streamable HTTP, stateless, answers as single JSON objects;
  MCP versions 2026-07-28 and 2025-03-26 to 2025-11-25. Requests from
  web pages of other sites are rejected (`Origin` check).

## Backups

Back up both volumes: the database (`pg_dump`) and the data directory with
the telemetry files.

```sh
docker compose -f docker/compose.yml exec postgres pg_dump -U stintory stintory > stintory.sql
docker run --rm -v stintory_data:/data -v "$PWD":/backup debian:trixie-slim \
    tar czf /backup/stintory-data.tgz -C /data .
```

## Privacy note

The server admin can technically read all data on the server (as with any
self-hosted service). Only use a server whose admin you trust, or run your
own.

## Development

```sh
docker compose -f docker/compose.dev.yml up -d   # Postgres on 127.0.0.1:55432
export STINTORY_TEST_DATABASE_URL=postgres://stintory:stintory@127.0.0.1:55432/stintory
cargo test -p sy-server                           # API tests, one schema each

npm run build -w web
STINTORY_DATABASE_URL=$STINTORY_TEST_DATABASE_URL STINTORY_SECRET_KEY=$(openssl rand -base64 32) \
STINTORY_ADMIN_USERNAME=admin STINTORY_ADMIN_PASSWORD='admin password 123' \
cargo run -p sy-server
```
