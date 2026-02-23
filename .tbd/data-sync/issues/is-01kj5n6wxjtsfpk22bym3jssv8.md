---
type: is
id: is-01kj5n6wxjtsfpk22bym3jssv8
title: Add per-epoch reads export to epoch race mapping table on stream details page
kind: feature
status: open
priority: 3
version: 2
labels: []
dependencies: []
created_at: 2026-02-23T16:27:08.080Z
updated_at: 2026-02-23T16:27:27.788Z
---
## Problem / Context

The stream details page (/streams/{streamId}) has whole-stream export endpoints (export.txt and export.csv) but no way to export reads scoped to a specific epoch. Operators need to download read data per-epoch for post-race analysis (e.g., to verify chip reads for a specific race mapped to that epoch). The epoch race mapping table is the natural place to surface per-row export actions.

The current reads API hardcodes current-epoch-only queries (reads.rs: e.stream_epoch = s.stream_epoch), so historical epoch reads are not accessible via export.

## Scope

- New backend endpoint for per-epoch reads export (CSV and/or text), following existing export.rs patterns
- New Export column in the epoch race mapping table on the stream details page, with download link(s) per epoch row
- Export should work for both the current epoch and historical epochs

## Non-Goals

- No changes to existing whole-stream export endpoints
- No pagination or dedup controls in this initial version — simple raw export
- No new database migrations needed (events table already stores stream_epoch)

## Acceptance Criteria

- [ ] New endpoint exists: GET /api/v1/streams/{stream_id}/epochs/{epoch}/export.csv (and/or .txt)
- [ ] Endpoint returns only reads from the specified stream_epoch value
- [ ] Epoch race mapping table has an Export column with download link(s) per epoch row
- [ ] Export links work for historical (non-current) epochs
- [ ] Export returns valid CSV with headers even when the epoch has zero reads
- [ ] Existing whole-stream export.txt and export.csv endpoints are unaffected

## Technical Notes

- services/server/src/http/export.rs: existing export pattern (streaming CSV/text) to follow closely
- services/server/src/http/reads.rs: current reads query hardcodes current epoch; new handler needs explicit epoch param
- services/server/src/lib.rs: register new route (see existing export route registration at lines 45-50)
- apps/server-ui/src/routes/streams/[streamId]/+page.svelte: epoch race mapping table at lines 603-741; add new Export column in row template (~line 659-720)
- apps/server-ui/src/lib/api.ts: add epochExportCsvUrl(streamId, epoch) and optionally epochExportTxtUrl() helpers

## Risks & Edge Cases

- Large epochs with many reads could produce large downloads — export.rs already uses streaming responses so this is manageable with the same pattern
- Handle case where epoch does not exist or belongs to a different stream: return 404
- Handle epoch with no reads: return valid empty CSV with headers only

## Validation Plan

- [ ] Assign a race to an epoch, generate reads, click Export CSV in table row — verify reads appear correctly
- [ ] Click Export for a historical (non-current) epoch — verify reads appear
- [ ] Click Export for an epoch with zero reads — verify empty but valid CSV
- [ ] Confirm existing whole-stream export.txt and export.csv still work
