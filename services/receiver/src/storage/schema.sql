-- Receiver local SQLite schema (v2)
-- Authority: receiver durable store schema.
-- Required PRAGMAs (set at connection open, not in this file):
--   PRAGMA journal_mode=WAL;
--   PRAGMA synchronous=FULL;
--   PRAGMA wal_autocheckpoint=1000;
--   PRAGMA foreign_keys=ON;
-- At startup, run PRAGMA integrity_check; exit if result != 'ok'.
--
-- Stream identity: except for `subscriptions` (which keeps separate
-- forwarder_endpoint_id + wire stream_id columns), every `stream_id` column in
-- this schema stores the receiver-local canonical stream key
-- `{forwarder_endpoint_id}\u{1F}{wire_stream_id}` (Unit Separator, U+001F; see
-- services/receiver/src/stream_key.rs). Two forwarders can expose the same
-- wire stream id (forwarder journal keys like `ip:port`), so the bare wire id
-- is ambiguous. Wire stream ids are arbitrary UTF-8 and never required to be
-- parseable UUIDs.
--
-- Versioning: `PRAGMA user_version = 1` is stamped when the schema is applied.
-- A database that has tables but user_version != 1 is rejected at startup: 0
-- predates canonical stream keys, and future versions must not be restamped down.

CREATE TABLE IF NOT EXISTS profile (
    server_url  TEXT NOT NULL,
    token       TEXT NOT NULL,
    update_mode TEXT NOT NULL DEFAULT 'check-and-download',
    receiver_mode_json TEXT,
    receiver_id TEXT,
    dbf_enabled INTEGER NOT NULL DEFAULT 0,
    -- Global announcer publish on/off (opt-in; default off).
    announcer_enabled INTEGER NOT NULL DEFAULT 0,
    announcer_max_list_size INTEGER NOT NULL DEFAULT 25,
    device_token TEXT,
    -- Race Director DBF participant/chip import (background poll). Opt-in;
    -- default off. The manual import action ignores `rd_import_enabled`.
    rd_import_enabled INTEGER NOT NULL DEFAULT 0,
    rd_import_dir     TEXT NOT NULL DEFAULT 'C:\Winrace\Files',
    rd_import_interval_secs INTEGER NOT NULL DEFAULT 15,
    -- DBF write coalescing interval (ms), clamped 250..5000 on save.
    dbf_flush_interval_ms INTEGER NOT NULL DEFAULT 1000
);

CREATE TABLE IF NOT EXISTS subscriptions (
    forwarder_endpoint_id TEXT NOT NULL,
    stream_id             TEXT NOT NULL,
    local_port_override   INTEGER,
    event_type            TEXT NOT NULL DEFAULT 'finish',
    forwarder_id          TEXT,
    reader_ip             TEXT,
    PRIMARY KEY (forwarder_endpoint_id, stream_id)
);

CREATE TABLE IF NOT EXISTS received_events (
    stream_id                 TEXT NOT NULL,
    seq                       BIGINT NOT NULL,
    epoch                     BIGINT NOT NULL,
    raw_frame                 BLOB NOT NULL,
    read_kind                 TEXT NOT NULL,
    reader_timestamp          TEXT,
    received_unix_ms          BIGINT NOT NULL,
    dbf_delivered_unix_ms     BIGINT,
    announcer_pushed_unix_ms  BIGINT,
    -- Chip id parsed once from raw_frame at persist time. NULL on rows
    -- persisted before the column existed (readers fall back to parsing).
    chip_id                   TEXT,
    PRIMARY KEY (stream_id, seq)
);

-- Retention watermark: seqs at or below pruned_through_seq have been deleted
-- from received_events. Durable-proxy consumers initialize their replay
-- cursor here (analogous to how gap markers jump the P2P cursor), so new
-- consumers are not stuck waiting for pruned seq 1.
CREATE TABLE IF NOT EXISTS retention (
    stream_id          TEXT PRIMARY KEY,
    pruned_through_seq BIGINT NOT NULL DEFAULT 0
);

-- Partial index so the DBF worker's undelivered probe/fetch is O(pending),
-- not O(stream rows). The set stays small at steady state because delivery
-- marks rows in batches right after each append pass.
CREATE INDEX IF NOT EXISTS idx_received_dbf_undelivered
    ON received_events(stream_id, seq)
    WHERE dbf_delivered_unix_ms IS NULL;

-- Fences the announcer push source. Holds the highest accepted
-- announcer_source_generation per stream so a delayed or out-of-order push
-- carrying an older generation can be rejected without sending stale rows.
CREATE TABLE IF NOT EXISTS announcer_source_fence (
    stream_id  TEXT PRIMARY KEY,
    generation BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS cursors (
    stream_id    TEXT PRIMARY KEY,
    last_seq     BIGINT NOT NULL,
    forwarder_id TEXT,
    reader_ip    TEXT,
    stream_epoch BIGINT
);

CREATE TABLE IF NOT EXISTS gap_markers (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    stream_id              TEXT NOT NULL,
    requested_after_seq    BIGINT NOT NULL,
    earliest_available_seq BIGINT NOT NULL,
    latest_available_seq   BIGINT NOT NULL,
    reason                 TEXT NOT NULL,
    created_unix_ms        BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_gap_markers_stream_created
    ON gap_markers (stream_id, created_unix_ms);

-- Earliest-epoch overrides keyed by canonical P2P stream_id. The optional
-- forwarder_id/reader_ip columns hold optional display metadata only when a
-- compatibility caller supplied it; canonical P2P callers leave them NULL so
-- no runtime fabricates a (forwarder_id, reader_ip) key from a stream_id.
CREATE TABLE IF NOT EXISTS earliest_epochs (
    stream_id             TEXT PRIMARY KEY,
    forwarder_endpoint_id TEXT NOT NULL,
    earliest_epoch        BIGINT NOT NULL,
    forwarder_id          TEXT,
    reader_ip             TEXT
);

-- Per-forwarder connect intent. Absence of a row means the default contract
-- (connect = true). A row with connect = 0 records an explicit disconnect
-- intent that must survive restarts but is cleared by a factory reset.
CREATE TABLE IF NOT EXISTS forwarder_intent (
    endpoint_id TEXT PRIMARY KEY,
    connect     INTEGER NOT NULL
);

-- Participant + chip-assignment data, imported from .ppl/.bibchip files, used
-- to resolve chip reads to bib/name locally for the announcer. Both are
-- replaced wholesale on import ("upload replaces all"). The bib is the
-- canonical i64 join key.
CREATE TABLE IF NOT EXISTS participants (
    bib         INTEGER PRIMARY KEY,
    last        TEXT NOT NULL,
    first       TEXT NOT NULL,
    affiliation TEXT NOT NULL,
    gender      TEXT NOT NULL,
    -- Division code (Race Director RUNDIV). NULL for .ppl imports. Joined to
    -- `divisions` for a display name on resolve.
    division    INTEGER
);

-- Division code -> display name, imported from Race Director DIVISION.DBF.
-- Replaced wholesale on import like `participants`. Additive: only RD imports
-- populate it; .ppl/.bibchip imports leave it empty (division resolves to NULL).
CREATE TABLE IF NOT EXISTS divisions (
    divno INTEGER PRIMARY KEY,
    name  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS bib_chips (
    chip_id TEXT PRIMARY KEY,
    bib     INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_bib_chips_bib ON bib_chips (bib);

-- Per-stream announcer publish opt-in. Presence of a stream_id means that
-- stream publishes to the announcer (when the global toggle is on). Kept in a
-- separate table so replacing the subscription set does not clobber the
-- per-stream publish choice. Opt-in: absent = does not publish.
CREATE TABLE IF NOT EXISTS announcer_publish_streams (
    stream_id TEXT PRIMARY KEY
);
