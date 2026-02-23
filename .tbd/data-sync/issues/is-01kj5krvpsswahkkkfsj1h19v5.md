---
type: is
id: is-01kj5krvpsswahkkkfsj1h19v5
title: Replace auto-save with explicit Save button on receiver UI selection card
kind: task
status: in_progress
priority: 2
version: 3
labels: []
dependencies: []
created_at: 2026-02-23T16:01:59.507Z
updated_at: 2026-02-23T18:05:19.644Z
---
**Problem:** Every control change in the Selection card (mode, race ID, epoch scope, replay policy, targeted replay rows) immediately fires applySelection() → PUT /api/v1/selection. An operator who changes Mode and then Race ID makes two separate PUT requests, with an intermediate half-configured state visible to the server between the two. An explicit Save button lets operators compose their full intent before committing.

**Scope:**
1. Dirty-state tracking — introduce a `savedPayload` state variable (JSON-serialized string of the last successfully written selection). Derive `isDirty = $derived(JSON.stringify(selectionPayload()) \!== savedPayload)`. Populate on initial load completion; update on successful save.
2. Decouple event handlers from applySelection() — each handle*Change handler should update its local state variable only. Remove all calls to applySelection() from those handlers.
3. Add Save button inside the Selection card — `disabled={\!isDirty || selectionBusy}`. Clicking it calls applySelection(). Existing selectionBusy disabling of all other controls remains for the duration of the in-flight request (double-click protection via existing coalescing logic is retained as-is).
4. Update tests — auto-apply tests become save-button tests: change a control → assert PUT not called → click Save → assert PUT called. New tests needed for Save button enabled/disabled state. Coalescing tests (lines 569, 603) can be simplified or replaced with a double-click guard test.

**Non-goals:**
- No Discard/Cancel button — reload to discard.
- No changes to selectionBusy coalescing internals — retain as double-click guard.
- selectedStreams (manual mode stream list, managed outside the Selection card) — out of scope; needs a separate decision.
- No changes to server API, receiver service, or any other app.

**Open question for implementer:** In manual mode, selectedStreams feeds into the selection payload. If changes to the stream list (outside the Selection card) currently also auto-save via applySelection(), those call sites are out of scope here but should be identified and either included or explicitly deferred.

**Acceptance criteria:**
- Changing any dropdown in the Selection card does NOT trigger a PUT request on its own.
- Save button is disabled on initial load (local state matches server state).
- Save button becomes enabled after any control change.
- Clicking Save fires PUT /api/v1/selection with the full current payload.
- Save button returns to disabled after a successful save with no further changes.
- Save button remains disabled while a save is in flight (selectionBusy).
- All other controls remain disabled while save is in flight (existing behaviour).
- Validation for targeted replay still runs on Save click; inline errors still shown.
- All existing tests pass (updated to use Save button where auto-apply was previously tested).
- New tests cover Save button enabled/disabled transitions.

**Technical notes:**
- Primary file: apps/receiver-ui/src/routes/+page.svelte
  - Lines ~58-73: add savedPayload state, derive isDirty
  - Lines ~441-499: strip applySelection() calls from all handle*Change handlers
  - Lines ~804-973 (Selection card markup): add Save button; bind disabled={\!isDirty || selectionBusy}
  - Line ~308-335 (loadAll): set savedPayload after server state is loaded
  - Line ~428 (applySelection): update savedPayload on successful PUT
- Test file: apps/receiver-ui/src/test/+page.svelte.test.ts
  - Rewrite tests at lines ~119, ~134, ~156 (auto-apply → save-on-click)
  - Simplify or replace coalescing tests at lines ~569, ~603
  - Add Save button enabled/disabled state tests

**Risks and edge cases:**
1. selectedStreams in manual mode — if any code path outside the Selection card calls applySelection() when the stream list changes, those paths are unaffected by this bead. Implementer should audit for this.
2. $derived over selectionPayload() — selectionPayload() reads multiple reactive state variables; confirm Svelte 5 $derived tracks all of them correctly (particularly targetedRows if it's an array/object).
3. Double-click on Save — existing coalescing logic (selectionBusy/selectionApplyQueued) handles this; no new logic needed.

**Validation plan:**
1. Run pnpm test in apps/receiver-ui — all tests must pass.
2. Manual: load receiver UI → confirm Save button disabled → change Mode → confirm enabled → click Save → confirm disabled again → change Race ID + Epoch Scope together → click Save once → confirm only one PUT fired with both changes.
3. Manual: targeted replay with invalid rows → click Save → confirm validation errors shown, no PUT fired.
