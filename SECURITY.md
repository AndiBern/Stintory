# Security policy

## Reporting a vulnerability

Please **don't open a public issue** for security problems. Report them privately through GitHub:
[Report a vulnerability](https://github.com/AndiBern/Stintory/security/advisories/new).

Include what you found, how to reproduce it, and which version and component (desktop app, server, web UI, `sy-bridge`) are affected. I'll confirm the report, keep you posted on the fix, and credit you in the advisory unless you'd rather stay anonymous.

Stintory is maintained by one person in their spare time, so there are no guaranteed response times.

## Supported versions

Only the latest release gets security fixes. Update the desktop app and the server image to the newest version before reporting.

## Scope

The most sensitive part is the self-hosted server: accounts, passwords, two-factor authentication, passkeys, sessions, OAuth for AI assistants, and the sharing of sessions and laps between users. Problems that let someone read or change another user's data, take over an account, or get around 2FA are the most important.

Out of scope:
- Problems that need an attacker who already controls your computer or your server.
- A server run without HTTPS or with a misconfigured reverse proxy (see [docs/SERVER.md](docs/SERVER.md)).
- Vulnerabilities in the games or their telemetry interfaces themselves.
