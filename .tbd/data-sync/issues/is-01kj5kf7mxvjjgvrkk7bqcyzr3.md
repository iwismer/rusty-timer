---
type: is
id: is-01kj5kf7mxvjjgvrkk7bqcyzr3
title: Rename 'Selection' card to 'Race & Mode Selection' in receiver UI
kind: task
status: open
priority: 3
version: 2
labels: []
dependencies: []
created_at: 2026-02-23T15:56:44.055Z
updated_at: 2026-02-23T15:56:58.253Z
---
**Problem:** The card grouping the receiver's mode and race controls is labelled 'Selection', which is too generic. Operators glancing at the UI don't immediately know what kind of selection the card governs.

**Scope:** Change `title="Selection"` → `title="Race & Mode Selection"` on the card component at `apps/receiver-ui/src/routes/+page.svelte:805`.

**Non-goals:**
- `data-testid="selection-section"` (line 806) — not changed; internal test handle only.
- Internal variable names (`selectionMode`, `selectionPayload`, etc.) — not changed.
- Docs/runbooks — not changed; headings there describe the concept, not the card title.

**Acceptance criteria:**
- The card displays 'Race & Mode Selection' as its visible title.
- No other visible text, layout, or behaviour is altered.
- Existing test suite (apps/receiver-ui) passes without modification.

**Technical notes:** Single-line change at apps/receiver-ui/src/routes/+page.svelte:805. No API, backend, or test-ID changes required.

**Validation:** Run pnpm test in apps/receiver-ui (all pass) + visual confirmation in browser.
