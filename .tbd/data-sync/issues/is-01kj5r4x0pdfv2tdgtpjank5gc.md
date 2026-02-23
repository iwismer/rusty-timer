---
type: is
id: is-01kj5r4x0pdfv2tdgtpjank5gc
title: Navigation from server admin page breaks — URL updates but UI stays frozen
kind: bug
status: in_progress
priority: 1
version: 3
labels: []
dependencies: []
created_at: 2026-02-23T17:18:28.372Z
updated_at: 2026-02-23T18:05:18.987Z
---
## Problem

Clicking any NavBar link while on the server admin page (/admin) leaves the UI frozen on the admin page. The URL in the address bar updates correctly (SvelteKit router processes the navigation), but the component tree does not transition — the admin page content remains visible. A hard refresh is required to land on the target route.

## Root Cause (Hypothesis — Needs Browser Verification)

The admin page is the **only** page in server-ui that renders a `ConfirmDialog`. That component uses `showModal()` on a native `<dialog>` element. `showModal()` places the dialog in the browser's **top layer** and marks all other document content as **inert** (pointer-events blocked, not focusable at the OS/browser level — not CSS-only).

The first `$effect` in `ConfirmDialog.svelte` has **no cleanup function**:

```javascript
$effect(() => {
  if (!dialogEl) return;
  if (open && !dialogEl.open) {
    dialogEl.showModal();
  } else if (!open && dialogEl.open) {
    dialogEl.close();
  }
});
```

There is no `return () => dialogEl.close()`. When the admin page component is destroyed (SvelteKit navigates away), Svelte runs effect cleanup — but since there is none, `dialogEl.close()` is never called. If the dialog was open at the moment of navigation, the top-layer/inert state persists in the browser even after the DOM node is removed, and the newly-rendered page content is effectively blocked from receiving input or rendering correctly.

**Secondary hypothesis:** Even when the dialog is closed, `<dialog class="fixed inset-0 ...">` is always present in the DOM. If Tailwind's CSS preflight does not preserve the browser's default `dialog:not([open]) { display: none }`, the element becomes an invisible full-viewport layer that silently absorbs clicks on NavBar links, preventing SvelteKit from intercepting them for client-side navigation.

## Reproduction Steps

1. Open server-ui → navigate to `/admin`
2. (Optional) click any destructive action button to open the ConfirmDialog, then cancel it
3. Click any NavBar link (Streams, Races, Logs)
4. Observe: URL bar updates to the new path, but page content does not change
5. Hard refresh: page correctly shows the target route

## Scope

- Fix is confined to `apps/shared-ui/src/components/ConfirmDialog.svelte` and/or `apps/server-ui/src/routes/admin/+page.svelte`
- All three UIs use `ConfirmDialog`, so the fix must not regress receiver-ui or forwarder-ui behavior
- No API, schema, or backend changes required

**Non-goals:**
- Redesigning the confirm dialog UX
- Changing any other admin page behavior

## Acceptance Criteria

- [ ] Navigating from `/admin` to any other server-ui route (Streams `/`, Races `/races`, Logs `/logs`) updates both the URL **and** the displayed page component without a refresh
- [ ] The fix is verified with the ConfirmDialog both in its default (never-opened) state and after having been opened and cancelled at least once
- [ ] No regression in ConfirmDialog behavior in receiver-ui (`/admin`) and forwarder-ui
- [ ] Browser console shows no uncaught errors during the navigation transitions
- [ ] `pnpm test` passes (or equivalent unit tests for ConfirmDialog if present)

## Technical Notes

**Key files:**
- `apps/shared-ui/src/components/ConfirmDialog.svelte` — add `return () => { if (dialogEl?.open) dialogEl.close(); }` to the first `$effect`; OR wrap with `{#if open}...{/if}` so the element is only mounted when needed
- `apps/server-ui/src/routes/admin/+page.svelte` — optionally change `<ConfirmDialog .../>` to `{#if confirmOpen}<ConfirmDialog .../>{/if}` to avoid mounting it until it is actually needed
- `apps/server-ui/svelte.config.js` — verify `fallback: "index.html"` is correct for SPA mode
- `apps/server-ui/src/routes/+layout.ts` — `prerender = true, ssr = false` (correct for static SPA)

**Investigation checklist before coding:**
1. Open browser DevTools > Elements while on /admin; check if `<dialog>` has `open` attribute after cancelling a confirm → if yes, `close()` is not being called
2. Check Network tab: after clicking a nav link, does SvelteKit issue any fetch requests for the new route? (Should see no server requests in SPA mode)
3. Check Console for JS errors during navigation
4. Inspect the `<dialog>` element CSS: confirm `display: none` is applied when `open` attribute is absent

**Likely fix (2–4 lines):**
In `ConfirmDialog.svelte`, add a cleanup return to the first `$effect`:
```javascript
$effect(() => {
  if (!dialogEl) return;
  if (open && !dialogEl.open) {
    dialogEl.showModal();
  } else if (!open && dialogEl.open) {
    dialogEl.close();
  }
  return () => {
    if (dialogEl?.open) dialogEl.close();
  };
});
```

## Risks & Edge Cases

- Calling `dialogEl.close()` during Svelte teardown may fire a `close` event on the dialog; ensure `handleCancel` / `onCancel` callback does not cause a state update on a destroyed component (could log a Svelte warning)
- If the secondary hypothesis (Tailwind preflight stripping `dialog:not([open])`) is the actual root cause, the fix is instead adding `hidden` class conditionally or explicitly setting `display: none` in CSS when closed
- The `ConfirmDialog` is shared across all three UIs — test all three after the fix

## Validation Plan

- [ ] Manual: reproduce the bug, apply fix, verify navigation from /admin to all other server-ui routes
- [ ] Manual: confirm ConfirmDialog still opens, confirms, and cancels correctly after fix
- [ ] Automated: run `pnpm -F @rusty-timer/server-ui test` (or equivalent) and confirm no regressions
