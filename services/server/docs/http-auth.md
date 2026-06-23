# Server HTTP routes & auth matrix

The server assumes a **Caddy + Authelia** reverse proxy terminates user
authentication in front of it. This document lists which routes the proxy must
protect. The in-code source of truth is the module doc on
`services/server/src/http/status.rs` and the `router` doc in
`services/server/src/http/mod.rs`.

| Route | Method | Auth posture | Enforced by |
| --- | --- | --- | --- |
| `/status` | GET | Public | none; no secrets exposed |
| `/healthz` | GET | Public | none |
| `/admin/devices/approve` | POST | Admin | upstream `Remote-User` header; requires `SERVER_TRUSTED_PROXY=1` |
| `/admin/enrollment-tokens` | GET | Admin | upstream `Remote-User` header; requires `SERVER_TRUSTED_PROXY=1` |
| `/admin/enrollment-tokens` | POST | Admin | upstream `Remote-User` header; requires `SERVER_TRUSTED_PROXY=1` |
| `/admin/enrollment-tokens/{token_id}/revoke` | POST | Admin | upstream `Remote-User` header; requires `SERVER_TRUSTED_PROXY=1` |
| `/register` | POST | M2M / device bearer | enrollment voucher, or the device's own minted token for idempotent re-register |
| `/forwarder/catalog` | POST | M2M / device bearer | matching forwarder minted token; approval not required |
| `/forwarders` | GET | M2M / device bearer | active receiver minted token |
| `/allowlist/receivers` | GET | M2M / device bearer | active forwarder minted token |
| `/announcer/rows` | POST | M2M / device bearer | active receiver minted token |
| `/announcer/takeover` | POST | M2M / device bearer | active receiver minted token |

## Caddy / Authelia requirements

- **Protect every `/admin/*` route.** Authelia must require an authenticated
  admin session and inject the identity header (`Remote-User`) on the proxied
  request. The server trusts a non-empty `Remote-User` only when
  `SERVER_TRUSTED_PROXY=1` is set.
- **Strip inbound `Remote-User` from untrusted clients.** Caddy must remove any
  client-supplied copy of the admin identity header before forwarding, so it
  cannot be spoofed.
- **Public routes** (`/status`, `/healthz`) may be allow-listed without
  authentication.
- **M2M/device routes** authenticate in-process with `Authorization: Bearer ...`
  and do not depend on the proxy. The proxy should forward the `Authorization`
  header unchanged and should not require a browser session for these routes.
- **Enrollment vouchers** are bootstrap/recovery secrets. Devices should persist
  the server-minted per-device token returned by `/register` and use that token
  for steady-state server requests.
