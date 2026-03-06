# UI Version & Build Date in Footer

## Summary

Display backend service version and UI build date in the footer of all 3 UIs (server-ui, receiver-ui, forwarder-ui).

## Format

`Rusty Timer · Server · v0.4.3 · Built 2026-03-06`

## Implementation

### 1. Vite build date injection (all 3 UIs)

Add `define: { __BUILD_DATE__: JSON.stringify(new Date().toISOString().split('T')[0]) }` to each `vite.config.ts`.

### 2. Server version endpoint (server only)

Add `GET /api/version` to the Axum server returning `{ "version": "0.4.3" }` using `env!("CARGO_PKG_VERSION")`. Forwarder and receiver already expose version via their existing APIs.

### 3. Footer updates (all 3 UIs)

Each `+layout.svelte` fetches version from its backend API on mount:

- server-ui: `GET /api/version`
- forwarder-ui: `GET /api/status` (already returns `version` field)
- receiver-ui: existing API endpoint

Display version + build date in footer. Graceful fallback if API unavailable (show build date without version).

### What stays the same

- No changes to shared-ui component library
- No changes to `package.json` versions
- Existing footer styling/layout pattern preserved
