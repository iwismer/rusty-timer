# Design: Show Current Seqnum in Admin Cursor Reset Table

## Summary

Enrich the existing `GET /api/v1/streams` response with `stream_epoch` and `acked_through_seq` fields from the `cursors` SQLite table, and add two new columns ("Epoch" and "Current Seq") to the admin page's Cursor Reset table.

## Backend Changes (services/receiver)

1. **`control_api.rs`** - In the `StreamEntry` struct, add two optional fields:
   - `stream_epoch: Option<u64>`
   - `acked_through_seq: Option<u64>`

2. **`control_api.rs`** - When building the streams response, load all cursors via the existing `db.load_cursors()`, build a lookup map keyed by `(forwarder_id, reader_ip)`, and attach matching cursor data to each stream entry. `None` when no cursor exists.

## Frontend Changes (apps/receiver-ui)

3. **`api.ts`** - Update the `StreamEntry` type to include `stream_epoch?: number` and `acked_through_seq?: number`.

4. **`+page.svelte`** - Add "Epoch" and "Current Seq" columns to the Cursor Reset table. Display the values when present, or a dash when null/undefined.

## What stays the same

- No new API endpoints
- No schema changes
- Reset functionality unchanged
- No changes to other tables on the admin page
