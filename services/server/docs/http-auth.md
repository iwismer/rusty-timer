# Server HTTP routes & auth matrix

The server assumes a **Caddy + Authelia** reverse proxy terminates user
authentication in front of it. This document is the authoritative list of which
routes the proxy must protect. The in-code source of truth is the module doc on
`services/server/src/http/status.rs` and the `router` doc in
`services/server/src/http/mod.rs`.

| Route | Method | Auth posture | Enforced by |
| --- | --- | --- | --- |
| `/status` | GET | Public | none (no secrets exposed) |
| `/healthz` | GET | Public | none |
| `/admin/devices/approve` | POST | Admin | upstream `Remote-User` header |
| `/admin/devices/rename` | POST | Admin | upstream `Remote-User` header |
| `/admin/enrollment-tokens` | GET | Admin | upstream `Remote-User` header |
| `/admin/enrollment-tokens` | POST | Admin | upstream `Remote-User` header |
| `/admin/enrollment-tokens/{token_id}/revoke` | POST | Admin | upstream `Remote-User` header |
| `/register` | POST | M2M / device bearer | provisioning bearer token, or non-revoked forwarder enrollment token for forwarder registration |
| `/forwarder/catalog` | POST | M2M / device bearer | provisioning bearer token, or registered non-revoked forwarder token |
| `/forwarders` | GET | M2M / device bearer | provisioning bearer token |
| `/allowlist/receivers` | GET | M2M / device bearer | provisioning bearer token, or any registered non-revoked forwarder token |
| `/announcer/rows` | POST | M2M / device bearer | provisioning bearer token |
| `/announcer/takeover` | POST | M2M / device bearer | provisioning bearer token |

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
- **M2M/device routes** authenticate in-process with `Authorization: Bearer ...`
  and do not depend on the proxy. The proxy should forward the `Authorization`
  header unchanged.
- **Forwarder enrollment tokens** are scoped to forwarders. Receiver tokens do
  not authorize forwarder catalog or receiver allow-list requests.
