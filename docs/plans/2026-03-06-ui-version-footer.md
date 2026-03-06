# UI Version Footer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Display backend service version and UI build date in the footer of all 3 UIs.

**Architecture:** Each UI's `vite.config.ts` injects `__BUILD_DATE__` at build time. Each backend exposes a `GET /api/v1/version` endpoint returning `{ "version": "x.y.z" }`. Each UI's `+layout.svelte` fetches version on mount and displays it in the footer as `Rusty Timer · Server · v0.4.3 · Built 2026-03-06`.

**Tech Stack:** Axum (Rust), SvelteKit, Vite, Tailwind CSS v4

---

### Task 1: Add version endpoint to server backend

**Files:**
- Modify: `services/server/src/http/mod.rs`
- Modify: `services/server/src/lib.rs`

**Step 1: Add the version module and handler**

In `services/server/src/http/mod.rs`, add at the end:

```rust
pub mod version;
```

Create `services/server/src/http/version.rs`:

```rust
use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct VersionResponse {
    pub version: &'static str,
}

pub async fn get_version() -> Json<VersionResponse> {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION"),
    })
}
```

**Step 2: Register the route**

In `services/server/src/lib.rs`, add after the `.route("/readyz", get(health::readyz))` line (~line 34):

```rust
.route("/api/v1/version", get(http::version::get_version))
```

**Step 3: Verify it compiles**

Run: `cargo check -p rt-server`

**Step 4: Commit**

```
feat(server): add GET /api/v1/version endpoint
```

---

### Task 2: Add version endpoint to receiver backend

**Files:**
- Modify: `services/receiver/src/control_api.rs`

**Step 1: Add handler and route**

Add the handler function near the other handlers in `services/receiver/src/control_api.rs`:

```rust
async fn get_version() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }))
}
```

Add the route in the router builder (after the `/api/v1/status` line, ~line 1364):

```rust
.route("/api/v1/version", get(get_version))
```

**Step 2: Verify it compiles**

Run: `cargo check -p rt-receiver`

**Step 3: Commit**

```
feat(receiver): add GET /api/v1/version endpoint
```

---

### Task 3: Add `__BUILD_DATE__` to all 3 Vite configs

**Files:**
- Modify: `apps/server-ui/vite.config.ts`
- Modify: `apps/receiver-ui/vite.config.ts`
- Modify: `apps/forwarder-ui/vite.config.ts`

**Step 1: Add define block to each vite.config.ts**

For each file, add a `define` property inside the `defineConfig({...})` object:

```ts
define: {
  __BUILD_DATE__: JSON.stringify(new Date().toISOString().split("T")[0]),
},
```

For `apps/server-ui/vite.config.ts`, add after the `plugins` array (after line 7):

```ts
define: {
  __BUILD_DATE__: JSON.stringify(new Date().toISOString().split("T")[0]),
},
```

For `apps/receiver-ui/vite.config.ts`, add after the `plugins` array (after line 7):

```ts
define: {
  __BUILD_DATE__: JSON.stringify(new Date().toISOString().split("T")[0]),
},
```

For `apps/forwarder-ui/vite.config.ts`, add after the `plugins` array (after line 6):

```ts
define: {
  __BUILD_DATE__: JSON.stringify(new Date().toISOString().split("T")[0]),
},
```

**Step 2: Commit**

```
feat(ui): inject __BUILD_DATE__ at Vite build time
```

---

### Task 4: Update server-ui footer

**Files:**
- Modify: `apps/server-ui/src/routes/+layout.svelte`

**Step 1: Add version fetch and update footer**

Add a `version` state variable and fetch it on mount. Update the footer to include version and build date.

In the `<script>` block, add after `let navLinks = ...` (~line 16):

```ts
let version = $state("");
```

Inside the existing `onMount(() => { ... })` callback, add at the top (after `initDarkMode();`):

```ts
fetch("/api/v1/version")
  .then((r) => r.json())
  .then((d: { version: string }) => { version = d.version; })
  .catch(() => {});
```

Replace the footer (lines 74-76) with:

```svelte
<footer class="border-t border-border py-3 px-6 text-center">
  <p class="text-xs text-text-muted m-0">
    Rusty Timer &middot; Server{version ? ` · v${version}` : ""} &middot; Built {__BUILD_DATE__}
  </p>
</footer>
```

**Step 2: Verify it builds**

Run: `cd apps/server-ui && npx vite build`

**Step 3: Commit**

```
feat(server-ui): show version and build date in footer
```

---

### Task 5: Update forwarder-ui footer

**Files:**
- Modify: `apps/forwarder-ui/src/routes/+layout.svelte`

**Step 1: Add version fetch and update footer**

The forwarder already exposes version via `GET /api/v1/status` → `{ version: "x.y.z", ... }`. But for consistency, use the same `/api/v1/version` pattern. The forwarder's status_http.rs already exposes version in the status endpoint, so we can also add a dedicated `/api/v1/version` route — OR we can just use the status endpoint since it already returns `version`. Let's use `/api/v1/status` since it already exists and the forwarder UI already calls it.

In the `<script>` block, add after `let currentPath = ...` (~line 12):

```ts
let version = $state("");
```

Add an `onMount` fetch (the existing `onMount` only calls `initDarkMode()`):

Update the existing `onMount` to also fetch version:

```ts
onMount(() => {
  initDarkMode();
  fetch("/api/v1/status")
    .then((r) => r.json())
    .then((d: { version: string }) => { version = d.version; })
    .catch(() => {});
});
```

Replace the footer (lines 31-33) with:

```svelte
<footer class="border-t border-border py-3 px-6 text-center">
  <p class="text-xs text-text-muted m-0">
    Rusty Timer &middot; Forwarder{version ? ` · v${version}` : ""} &middot; Built {__BUILD_DATE__}
  </p>
</footer>
```

**Step 2: Verify it builds**

Run: `cd apps/forwarder-ui && npx vite build`

**Step 3: Commit**

```
feat(forwarder-ui): show version and build date in footer
```

---

### Task 6: Update receiver-ui footer

**Files:**
- Modify: `apps/receiver-ui/src/routes/+layout.svelte`

**Step 1: Add version fetch and update footer**

In the `<script>` block, add after `let { children } = $props();` (~line 8):

```ts
let version = $state("");
```

Update the existing `onMount` to also fetch version:

```ts
onMount(() => {
  initDarkMode();
  fetch("/api/v1/version")
    .then((r) => r.json())
    .then((d: { version: string }) => { version = d.version; })
    .catch(() => {});
});
```

Replace the footer (lines 37-39) with:

```svelte
<footer class="border-t border-border py-3 px-8 text-center">
  <p class="text-xs text-text-muted m-0">
    Rusty Timer &middot; Receiver{version ? ` · v${version}` : ""} &middot; Built {__BUILD_DATE__}
  </p>
</footer>
```

**Step 2: Verify it builds**

Run: `cd apps/receiver-ui && npx vite build`

**Step 3: Commit**

```
feat(receiver-ui): show version and build date in footer
```

---

### Task 7: Add TypeScript declaration for __BUILD_DATE__

**Files:**
- Create or modify: `apps/server-ui/src/global.d.ts` (or `app.d.ts` if it exists)
- Create or modify: `apps/receiver-ui/src/global.d.ts`
- Create or modify: `apps/forwarder-ui/src/global.d.ts`

**Step 1: Declare the global constant**

In each app, ensure there's a declaration:

```ts
declare const __BUILD_DATE__: string;
```

Check if `app.d.ts` or `global.d.ts` already exists in each app's `src/` directory first. If `app.d.ts` exists, add the declaration there. Otherwise create `src/global.d.ts`.

**Step 2: Commit**

```
feat(ui): add TypeScript declaration for __BUILD_DATE__
```
