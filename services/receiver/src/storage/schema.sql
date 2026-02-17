-- Receiver local SQLite schema
-- Event cache for timing data received from the server.
--
-- Required PRAGMAs (set at connection open, not in this file):
--   PRAGMA journal_mode=WAL;
--   PRAGMA synchronous=FULL;
--   PRAGMA wal_autocheckpoint=1000;
--   PRAGMA foreign_keys=ON;
--
-- At startup, run PRAGMA integrity_check; exit if result != 'ok'.

CREATE TABLE IF NOT EXISTS event_cache (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    forwarder_id TEXT NOT NULL,
    reader_ip    TEXT NOT NULL,
    stream_epoch INTEGER NOT NULL,
    seq          INTEGER NOT NULL,
    reader_timestamp TEXT NOT NULL,
    raw_read_line TEXT NOT NULL,
    read_type    TEXT NOT NULL,
    received_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(forwarder_id, reader_ip, stream_epoch, seq)
);

CREATE TABLE IF NOT EXISTS stream_cursors (
    forwarder_id TEXT NOT NULL,
    reader_ip    TEXT NOT NULL,
    stream_epoch INTEGER NOT NULL,
    last_seq     INTEGER NOT NULL,
    updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (forwarder_id, reader_ip)
);
