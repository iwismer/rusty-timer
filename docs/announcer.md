# Announcer

## Overview

The announcer is a thin-node status feature for displaying recent finishers on
a public screen. Receivers push announcer rows to the thin node with an
idempotency key and fenced generation. The thin node stores the current board
state and serves read-only public status endpoints.

The chip-read data plane remains direct forwarder-to-receiver P2P. The thin
node only receives the sanitized row data that the receiver chooses to publish.

## How It Works

1. The receiver subscribes to one or more finish-line streams over iroh.
2. The receiver durably writes reads and updates its local DBF/TCP outputs.
3. When announcer push is enabled, the receiver emits sanitized rows to the
   thin node.
4. The thin node accepts rows only for the active fenced generation and ignores
   duplicate idempotency keys.
5. Public display clients read the thin-node status board.

## Key Behaviors

- **Receiver-owned publishing.** The receiver decides which streams and event
  types feed the announcer.
- **Idempotent row push.** Retries use the same idempotency key and do not
  create duplicate rows.
- **Fenced generation.** Older receiver generations are rejected so stale
  processes cannot overwrite current announcer state.
- **Thin-node persistence.** Board state is stored in the thin-node SQLite
  database and survives process restarts.
- **No chip-read relay.** The thin node does not store or forward the raw event
  journal.

## Operations

Use the receiver UI or receiver control API to enable/disable announcer push and
select the source streams. Use the thin-node status board to inspect current
rows, endpoint registrations, and allow-list state.

For thin-node provisioning, auth posture, and recovery procedures, see
[Thin-node operations](runbooks/thin-node-operations.md). For the race-day
workflow, see the [race-day operator guide](runbooks/race-day-operator-guide.md).
