# Thin-node HTTP routes & auth matrix

The thin node assumes a **Caddy + Authelia** reverse proxy terminates user
authentication in front of it. This document is the authoritative list of which
routes the proxy must protect. The in-code source of truth is the module doc on
`services/thin-node/src/http/status.rs` and the `router` doc in
`services/thin-node/src/http/mod.rs`.

| Route                        | Method | Auth posture        | Enforced by                  |
| ---------------------------- | ------ | ------------------- | ---------------------------- |
| `/status`                    | GET    | Public              | none (no secrets exposed)    |
| `/healthz`                   | GET    | Public              | none                         |
| `/admin/devices/approve`     | POST   | Admin               | upstream `Remote-User` header|
| `/register`                  | POST   | M2M / device bearer | in-process provisioning bearer token |
| `/announcer/rows`            | POST   | M2M / device bearer | in-process provisioning bearer token |
| `/announcer/takeover`        | POST   | M2M / device bearer | in-process provisioning bearer token |

## Caddy / Authelia requirements

- **Protect every `/admin/*` route.** Authelia must require an authenticated
  admin session and inject the identity header (`Remote-User`) on the proxied
  request. The node trusts a non-empty `Remote-User` as proof of an
  authenticated admin.
- **Strip inbound `Remote-User` from untrusted clients.** Caddy must remove any
  client-supplied copy of the admin identity header before forwarding, so it
  cannot be spoofed.
- **Public routes** (`/status`, `/healthz`) may be allow-listed without
  authentication.
- **M2M/device routes** (`/register`, `/announcer/*`) authenticate in-process
  using the provisioning bearer token and do not depend on the proxy; the proxy
  should forward the `Authorization` header unchanged.

> Allow-list distribution to devices is intentionally **not** implemented at
> this stage; this document only covers the status/admin/M2M route auth matrix.
