---
type: is
id: is-01kj5ppr9bc58pfhg5ac8abrgg
title: Remove redundant Clear button from epoch name column in forwarder UI readers table
kind: task
status: closed
priority: 3
version: 3
labels: []
dependencies: []
created_at: 2026-02-23T16:53:16.202Z
updated_at: 2026-02-23T19:31:44.946Z
closed_at: 2026-02-23T19:31:44.945Z
close_reason: "PR #100 created. Clear button removed from forwarder UI epoch name column."
---
## Problem

The epoch name column in the forwarder UI readers table shows three controls: a text input, a "Save" button, and a "Clear" button. The "Clear" button is redundant: clearing is already achievable by emptying the input field and clicking Save (the handler converts empty string to `null`). Two buttons for one field creates unnecessary visual clutter.

## Scope

**In scope:** Remove the "Clear" button only. No other changes.

**Out of scope:** Changes to the handler, the input field, the Save button, or any other column.

## Location

`apps/forwarder-ui/src/routes/+page.svelte:436-443`

```svelte
<button
  onclick={() =>
    handleSetCurrentEpochName(reader.ip, null)}
  class="..."
  disabled={epochNameBusy[reader.ip] === true}
>
  Clear
</button>
```

Delete these 8 lines. The `handleSetCurrentEpochName` function and the null-handling path can remain unchanged.

## Acceptance Criteria

- [ ] The "Clear" button is no longer rendered in the epoch name column.
- [ ] The text input and "Save" button remain functional.
- [ ] Saving an empty input still clears the epoch name (no regression).
- [ ] No other UI changes.

## Validation

- Manual: open forwarder UI readers table, confirm only input + Save are present in the epoch name column.
- Manual: clear an epoch name by emptying the input and clicking Save, confirm it works.
- Automated: update any snapshot/component test that references the Clear button.
