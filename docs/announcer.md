# Announcer

## Overview

The announcer is a live, public-facing screen served at `/announcer` that shows
recent finishers during a race. It is built into `rt-server` and requires no
additional services. Point a projector, TV, or tablet at the announcer URL and
spectators see finisher names as they cross the line.

## How It Works

When enabled, the announcer listens to a selected stream and displays chip reads
enriched with participant names (from the server's participant list). Each new
unique chip crossing produces a row on the announcer screen with the
participant's display name, bib number, and a running finisher count.

The public announcer page (`/announcer`) connects to a sanitized SSE endpoint
and updates in real time. An operator configures which stream to follow via the
dashboard at `/announcer-config` or through the REST API.

## Key Behaviors

- **Requires a selected stream.** The announcer does nothing until an operator
  picks a stream in the configuration.
- **24-hour expiry on enable.** Once enabled, the announcer automatically
  disables itself after 24 hours. Re-enable it if the event spans multiple days.
- **In-memory state resets on restart.** The finisher list and seen-chip set live
  in memory. Restarting `rt-server` clears them. There is no backfill from the
  database.
- **No backfill.** The announcer only shows chip reads that arrive while it is
  enabled and a stream is selected. Historical reads are not replayed.
- **Deduplication by chip ID.** Each chip is shown at most once per session. A
  reset clears the seen set and allows the same chips to appear again.

## Configuration

Configure the announcer through the dashboard UI at `/announcer-config` or via
the REST API (`GET`/`PUT` on `/api/v1/announcer/config`). Configuration is
persisted in Postgres and survives restarts.

Settings:

| Field       | Description                                  |
|-------------|----------------------------------------------|
| `enabled`   | Master on/off switch                         |
| `stream_id` | UUID of the stream to follow                 |

## Endpoints

### Public (safe to expose without auth)

These endpoints serve sanitized data with no internal IDs. They are safe to
expose through a reverse proxy without authentication.

| Method | Path                                  | Description              |
|--------|---------------------------------------|--------------------------|
| GET    | `/announcer`                          | Public announcer screen  |
| GET    | `/api/v1/public/announcer/state`      | Sanitized state snapshot |
| GET    | `/api/v1/public/announcer/events`     | Sanitized SSE updates    |

### Operator / Internal (keep behind auth)

These endpoints expose configuration controls and full state including internal
IDs. Keep them behind authentication.

| Method   | Path                          | Description                   |
|----------|-------------------------------|-------------------------------|
| GET      | `/announcer-config`           | Configuration UI              |
| GET/PUT  | `/api/v1/announcer/config`    | Read/write announcer config   |
| POST     | `/api/v1/announcer/reset`     | Reset runtime state           |
| GET      | `/api/v1/announcer/state`     | Full snapshot (internal IDs)  |
| GET      | `/api/v1/announcer/events`    | Full SSE updates              |

## Reset Triggers

The announcer runtime state (finisher list, seen-chip set, finisher count) is
cleared in the following situations:

- **Manual reset** — `POST /api/v1/announcer/reset`
- **Stream selection change** — switching to a different stream resets state
- **Epoch change** — a new stream epoch from the forwarder triggers a reset
- **Server restart** — in-memory state does not survive restarts

After a reset, connected SSE clients receive a resync event and the announcer
page refreshes automatically.

## Race-Day Usage

1. Upload or verify the participant list before the race.
2. Open `/announcer-config` and select the stream for the finish line reader.
3. Enable the announcer.
4. Point the display device at `/announcer`.
5. After the race, disable the announcer or let the 24-hour expiry handle it.

For the full start-to-finish race-day workflow, see the
[race-day operator guide](runbooks/race-day-operator-guide.md).

## Reverse Proxy Notes

In production, place a reverse proxy in front of `rt-server` so that:

- Public announcer paths (`/announcer`, `/api/v1/public/announcer/*`) are open.
- Operator paths (`/announcer-config`, `/api/v1/announcer/*`) require
  authentication.

See the Caddy + Authelia example in
[`deploy/server/README.md`](../deploy/server/README.md) for a working
configuration that implements this split.
