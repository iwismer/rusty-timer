# PiSugar 3 Plus UPS Support

**Date:** 2026-04-01
**Status:** Approved
**Scope:** Forwarder, rt-protocol, server, receiver UI, dashboard, SBC configurator

## Overview

Add optional PiSugar 3 Plus UPS monitoring to the forwarder. Battery status
(percent, voltage, charging, power plugged, temperature) flows from the
forwarder through the server to the receiver and dashboard UIs, giving the
operator real-time visibility into forwarder power state during races.

The forwarder communicates with the PiSugar hardware via the official
`pisugar-server` daemon's TCP text protocol on `127.0.0.1:8423`. The daemon
handles all hardware concerns (I2C, safe shutdown, auto power-on). The
forwarder is a read-only client.

## Data Model & Protocol

### Shared types (rt-protocol)

```rust
/// Raw UPS telemetry. Integer types avoid float comparison issues.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsStatus {
    pub battery_percent: u8,           // 0-100
    pub battery_voltage_mv: u16,       // millivolts, e.g. 3870 = 3.87V
    pub charging: bool,
    pub power_plugged: bool,
    pub temperature_cdeg: i16,         // centi-degrees C, e.g. 4200 = 42.00°C
    pub sampled_at: i64,               // unix epoch millis when forwarder polled the daemon
}
```

UIs divide voltage by 1000 and temperature by 100 for display.

#### Change detection

Change detection compares all fields *except* `sampled_at` (which changes
every poll). A helper method `UpsStatus::same_readings(&self, other: &Self)`
compares `battery_percent`, `battery_voltage_mv`, `charging`, `power_plugged`,
and `temperature_cdeg` only. This drives the "send on change vs heartbeat"
logic.

### UPS availability state model

The forwarder, server, and UIs must distinguish three states:

| State | Meaning | Forwarder behavior | Server/UI behavior |
|---|---|---|---|
| `not_configured` | `[ups]` absent or `enabled = false` | No UPS task spawned | No UPS indicator shown |
| `available` | Daemon reachable, status known | Carries `UpsStatus` | Show live battery indicator |
| `unavailable` | Daemon unreachable or erroring | Carries `last_status: Option<UpsStatus>` | Show stale indicator with "last seen" from `sampled_at`, or "UPS unavailable" if no prior sample |

When a forwarder disconnects from the server, the server clears its UPS cache.
The UI shows the forwarder as offline (existing behavior); no stale UPS data
is displayed for offline forwarders.

### New WsMessage variant (forwarder -> server)

```rust
ForwarderUpsStatus {
    forwarder_id: String,
    available: bool,              // false = daemon unreachable
    status: Option<UpsStatus>,    // present when available, or last known when unavailable
}
```

A single variant carries both availability and data. When `available` is false
and `status` is `Some`, the UI can show stale data with a warning. When
`available` is false and `status` is `None`, the daemon has never been
reachable since the forwarder started.

### Server fanout (no new WS variant for receivers)

UPS status reaches downstream consumers via existing transport mechanisms:

**Dashboard**: New `DashboardEvent::ForwarderUpsUpdated` variant, sent on the
existing `dashboard_tx` SSE channel. No new polling endpoint needed — the
dashboard subscribes to SSE and receives UPS updates alongside existing
forwarder metrics.

```rust
DashboardEvent::ForwarderUpsUpdated {
    forwarder_id: String,
    available: bool,
    status: Option<UpsStatus>,
}
```

**Receiver**: New sentinel `ReadEvent` with read type
`__forwarder_ups_status`. The JSON-serialized payload is placed in
`raw_frame`, following the same pattern as `__reader_status_changed` and
`__reader_info_updated`. Broadcast on one of the forwarder's stream channels
(any stream — the receiver deduplicates by `forwarder_id`).

```rust
pub const FORWARDER_UPS_STATUS_READ_TYPE: &str = "__forwarder_ups_status";
```

**Initial snapshot for newly connected receivers**: When a receiver connects
and subscribes to streams, the server sends the current cached UPS status (if
any) for each forwarder that has streams in the receiver's subscription. This
is sent as a sentinel `ReadEvent` on the first subscribed stream for each
forwarder, ensuring the receiver doesn't wait up to 60s for the first update.

### Send behavior

- Forwarder polls pisugar-server daemon every 5 seconds
- Sends `ForwarderUpsStatus` immediately when readings change (per
  `same_readings()`)
- Sends a periodic heartbeat every 60 seconds even if no readings changed
  (freshness signal — updates `sampled_at`)
- Sends a full `ForwarderUpsStatus` immediately after `ForwarderHello` on WS
  reconnect so the server's cache is repopulated without waiting up to 60s
- Sends `ForwarderUpsStatus { available: false, status: last_known }` when the
  daemon becomes unreachable, and `{ available: true, status }` when it
  recovers

The 5-command TCP sequence per poll (battery, battery_v, battery_charging,
battery_power_plugged, temperature) is 5 round trips over localhost. At a 5s
interval this is negligible. The pisugar daemon does not support batch
commands. Command names match the daemon's actual protocol: `get battery`,
`get battery_v`, `get battery_charging`, `get battery_power_plugged`,
`get temperature`.

### Backwards compatibility

The new `ForwarderUpsStatus` WsMessage kind is additive. Old servers that
don't understand it will hit the unknown-variant path and ignore it. The
sentinel `ReadEvent` approach for receiver fanout requires no protocol changes
— receivers that don't recognize `__forwarder_ups_status` skip it like any
unknown sentinel.

## Forwarder

### Config

New optional TOML section:

```toml
[ups]
enabled = false                     # default: no UPS
daemon_addr = "127.0.0.1:8423"     # pisugar-server TCP port
poll_interval_secs = 5
upstream_heartbeat_secs = 60
```

All fields have defaults. The `[ups]` section is optional — absent means
disabled. Exposed through the existing config editing API
(`POST /api/v1/config/ups`) and editable from the receiver's ForwarderConfig
panel. The request body mirrors the TOML fields as JSON. Validation:
`daemon_addr` must be a valid `host:port`, `poll_interval_secs` must be 1-60,
`upstream_heartbeat_secs` must be 10-300.

### New module: `pisugar_client.rs`

Async TCP client using `tokio::net::TcpStream`:

- Sends plain-text commands: `get battery\n`, `get battery_v\n`,
  `get battery_charging\n`, `get battery_power_plugged\n`, `get temperature\n`
- Parses plain-text responses (e.g. `battery: 73\n`, `battery_v: 3.87\n`,
  `battery_charging: true\n`, `battery_power_plugged: true\n`,
  `temperature: 42.5\n`)
- Converts float responses to integer types at the parse boundary:
  `battery_v` `"3.87"` -> `3870u16` millivolts;
  `temperature` `"42.5"` -> `4250i16` centi-degrees
- Reconnects on connection loss with exponential backoff (1s base, 30s cap)
- Exposes `poll_status() -> Result<UpsStatus>`

### Background task

Spawned in `main.rs` alongside the update checker when `[ups] enabled = true`:

- Runs a `tokio::time::interval(poll_interval_secs)` loop
- Calls `pisugar_client.poll_status()`
- On success: stamps `sampled_at` with current unix epoch millis, compares
  with previous status using `same_readings()`:
  - Readings changed: broadcasts `ForwarderUiEvent::UpsStatusChanged` via SSE
    and sends `WsMessage::ForwarderUpsStatus { available: true, status }`
  - Readings unchanged: sends upstream only when the 60s heartbeat timer fires
- On error (daemon unreachable): sends
  `ForwarderUpsStatus { available: false, status: last_known }` once on
  transition, then suppresses until recovery
- Logs `warn!` when `power_plugged` transitions to `false` (fires once on
  transition, not every poll)
- Logs `warn!` when `battery_percent` crosses below 20% (fires once on
  crossing, not every poll while below)
- On WS uplink reconnect: immediately sends the latest UPS state

### New ForwarderUiEvent variant

```rust
UpsStatusChanged {
    available: bool,
    status: Option<rt_protocol::UpsStatus>,
}
```

### SubsystemStatus

New field: `ups_status: Option<UpsStatus>`. Included in `/api/v1/status` JSON.
`None` when UPS is not configured. When configured, reflects current
availability state.

## Server

### ws_forwarder.rs

New match arm for `WsMessage::ForwarderUpsStatus`:

1. Look up the forwarder's `device_id` and stream IDs from session state
2. Cache the latest UPS state (available + status) keyed by `forwarder_id` in
   a new `ForwarderUpsCache` (`Arc<RwLock<HashMap<String, CachedUpsState>>>`)
   on `AppState`
3. Emit `DashboardEvent::ForwarderUpsUpdated` on `dashboard_tx`
4. If `status` is `Some`, serialize as a sentinel `ReadEvent` with read type
   `__forwarder_ups_status` and broadcast on one of the forwarder's stream
   channels
5. On `power_plugged` transition (compare with cached value): write a row to
   `forwarder_ups_events`

On forwarder WS disconnect: clear the forwarder's entry from
`ForwarderUpsCache` and emit a `DashboardEvent::ForwarderUpsUpdated` with
`available: false, status: None` so the dashboard clears the indicator.

### Transition logging

Lightweight persistence for post-race diagnostics. Only `power_plugged`
transitions are logged, not every poll.

New migration:

```sql
CREATE TABLE forwarder_ups_events (
    id              BIGSERIAL PRIMARY KEY,
    forwarder_id    TEXT NOT NULL,
    event_type      TEXT NOT NULL CHECK (event_type IN ('power_lost', 'power_restored')),
    battery_percent SMALLINT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_ups_events_forwarder ON forwarder_ups_events(forwarder_id, created_at);
```

Uses `forwarder_id TEXT` to match the existing schema convention (no foreign
key to a non-existent `devices` table). The server writes a row when it
detects a `power_plugged` transition by comparing the incoming status with the
cached value.

### Receiver initial snapshot

When a receiver connects and the server builds its stream subscriptions, for
each unique `forwarder_id` in the subscription set, the server checks
`ForwarderUpsCache`. If a cached UPS state exists, it sends a sentinel
`ReadEvent` with `__forwarder_ups_status` on the first subscribed stream for
that forwarder. This ensures the receiver has UPS data immediately without
waiting for the next poll cycle.

## UIs

### Receiver UI — ForwardersTab (list view)

New battery column between forwarder name and readers columns:

- Battery icon with fill level proportional to `battery_percent`
- Color-coded: green (>50%), yellow (20-50%), red (<20%)
- Lightning bolt overlay when `charging` is true
- "—" when the forwarder has no UPS (`not_configured`)
- Grayed out with "UPS unavailable" tooltip when `available` is false

### Receiver UI — ForwardersTab (detail view)

New stats card in the existing grid (5th card):

- Label: "Battery"
- Large number: `battery_percent%`
- Sub-text: `voltage V` / `temperature °C` (converted from integer types)
- Status line: "Charging", "On Battery", or "Plugged In" (plugged but not
  charging = full)
- "Last updated Xs ago" sub-text derived from `sampled_at`

### Receiver UI — StreamsTab

Small battery icon next to the forwarder name/status dot in each stream row.
Same color coding. Provides at-a-glance battery awareness without leaving
streams view.

### Receiver UI — Critical battery warning

Persistent warning banner at the top of the receiver UI when any connected
forwarder has `battery_percent` below 15% and `power_plugged` is false.
Banner text: "Low battery: {forwarder_name} at {percent}%". Dismissible but
reappears if the condition persists on the next update. Multiple low-battery
forwarders are listed together.

### Dashboard — Forwarders page

Same battery column in list view and stats card in detail view as receiver UI.
Data arrives via the existing `DashboardEvent` SSE channel
(`ForwarderUpsUpdated` events). Same critical battery warning banner.

### Forwarder local UI

Battery indicator in the status bar area (alongside ready/uplink status).
Shows percent, charging state, and a warning banner when on battery power.
Shows "UPS unavailable" when daemon is unreachable.

### Store changes

Both receiver and dashboard stores get a
`upsState: Map<forwarder_id, { available: boolean, status: UpsStatus | null }>`
field. Receiver updates from sentinel `ReadEvent` messages on the WS.
Dashboard updates from `ForwarderUpsUpdated` SSE events.

## SBC Configurator

### rt-setup.sh

New interactive prompt after existing setup:

```
Do you have a PiSugar UPS HAT installed? [y/N]
```

If yes:

1. **Enable I2C**: Add `dtparam=i2c_arm=on` to `/boot/config.txt` (idempotent)
2. **Install pisugar-server**: Download `.deb` from PiSugar GitHub releases,
   pinned to a known-good version. The version is defined as a variable at the
   top of the script (`PISUGAR_SERVER_VERSION="1.7.8"`) for easy bumping. Uses
   the same GitHub API + arch detection pattern as the forwarder binary
   download.
3. **Configure pisugar-server**: Write `/etc/pisugar-server/config.json`:
   - `safe_shutdown_level: 10`
   - `safe_shutdown_delay: 30`
   - `auto_power_on: true`
   - `soft_poweroff: true`
   - `soft_poweroff_shell: "shutdown --poweroff 0"`
4. **Enable and start** `pisugar-server.service`
5. **Systemd ordering**: Add `After=pisugar-server.service` to
   `rt-forwarder.service`
6. **Update forwarder.toml**: Add `[ups] enabled = true`

Noninteractive mode: `RT_SETUP_UPS_ENABLED=1` environment variable.

### sbc_cloud_init.py

New optional field: `ups_enabled: bool = False`

When true:

- Adds `RT_SETUP_UPS_ENABLED=1` to the generated `rt-setup.env`
- Adds `i2c-tools` to the cloud-init package list

### Dashboard SBC setup web UI

New "PiSugar UPS" toggle in the configuration form. When enabled, shows a
brief note: "Installs pisugar-server and configures safe shutdown. Requires
PiSugar 3 HAT." Sets `ups_enabled` in the generated cloud-init files.

## Testing

### Forwarder unit tests (pisugar_client.rs)

- Mock TCP server responding to the text protocol
- Parsing of all 5 data points from daemon responses, including float-to-int
  conversion (e.g. `"3.87"` -> `3870u16`)
- Reconnect behavior on connection drop
- `same_readings()` correctly ignores `sampled_at` differences
- Change detection (only sends upstream when readings change)
- 60s heartbeat fires when readings are stable
- Immediate send on WS reconnect
- `available: false` sent once on daemon disconnect, not repeated every poll
- Warning logs fire on transition only, not every poll

### Forwarder integration tests

- UPS config absent: no UPS task spawned, no `ups_status` in status JSON
- UPS enabled, daemon unreachable: graceful degradation, `ups_status` present
  with `available: false`, warn logs
- UPS enabled, daemon available: `ups_status` populated with `available: true`,
  SSE events emitted, `sampled_at` is recent

### Protocol tests (rt-protocol)

- Round-trip serde for `ForwarderUpsStatus`
- `same_readings()` returns true when only `sampled_at` differs
- Sentinel `ReadEvent` with `__forwarder_ups_status` round-trips correctly

### Server tests

- `ForwarderUpsStatus` received: `DashboardEvent::ForwarderUpsUpdated` emitted,
  sentinel `ReadEvent` broadcast on stream channel
- `ForwarderUpsCache` cleared on forwarder disconnect
- `power_plugged` transition writes row to `forwarder_ups_events`
- Receiver initial snapshot includes cached UPS state for subscribed forwarders
- Multi-reader forwarder: sentinel broadcast sent on one stream only, not
  duplicated per stream

### UI tests (vitest)

- ForwardersTab renders battery indicator with UPS data
- ForwardersTab renders "—" without UPS data (not configured)
- ForwardersTab renders "UPS unavailable" when `available` is false
- Battery color coding thresholds (green/yellow/red)
- Critical battery warning banner appears at <=15% on battery
- Staleness display from `sampled_at`

### SBC configurator tests

- `rt-setup.sh` with `RT_SETUP_UPS_ENABLED=1` produces correct TOML and
  systemd unit
- Cloud-init generator with `ups_enabled=true` includes correct env vars and
  packages

### Not in scope

- Chaos or power-loss tests (pisugar-server daemon owns the safe shutdown path)
- Audible alerts (no audio notification system exists; future enhancement)
- Historical UPS telemetry beyond power transitions (battery % over time, etc.)
