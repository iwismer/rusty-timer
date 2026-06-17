-- Receiver local SQLite schema (v2)
-- Authority: receiver durable store schema.
-- Required PRAGMAs (set at connection open, not in this file):
--   PRAGMA journal_mode=WAL;
--   PRAGMA synchronous=FULL;
--   PRAGMA wal_autocheckpoint=1000;
--   PRAGMA foreign_keys=ON;
-- At startup, run PRAGMA integrity_check; exit if result != 'ok'.
--
-- P2P stream IDs are stored as canonical UUID TEXT (lowercase hyphenated form).
-- Legacy receiver APIs populate compatibility columns where present so the
-- incremental migration can keep existing callers green while new P2P APIs use
-- stream_id directly.

CREATE TABLE IF NOT EXISTS profile (
    server_url  TEXT NOT NULL,
    token       TEXT NOT NULL,
    update_mode TEXT NOT NULL DEFAULT 'check-and-download',
    receiver_mode_json TEXT,
    receiver_id TEXT,
    dbf_enabled INTEGER NOT NULL DEFAULT 0,
    dbf_path    TEXT NOT NULL DEFAULT 'C:\winrace\Files\IPICO.DBF'
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
    PRIMARY KEY (stream_id, seq)
);

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
-- forwarder_id/reader_ip columns hold real legacy metadata only when a legacy
-- WS caller created the row; canonical P2P callers leave them NULL so the
-- legacy runtime never fabricates a (forwarder_id, reader_ip) key from a
-- stream_id.
CREATE TABLE IF NOT EXISTS earliest_epochs (
    stream_id             TEXT PRIMARY KEY,
    forwarder_endpoint_id TEXT NOT NULL,
    earliest_epoch        BIGINT NOT NULL,
    forwarder_id          TEXT,
    reader_ip             TEXT
);
