-- Forwarder local SQLite schema
-- Durable journal for events read from IPICO timing hardware.
--
-- Required PRAGMAs (set at connection open, not in this file):
--   PRAGMA journal_mode=WAL;
--   PRAGMA synchronous=FULL;
--   PRAGMA wal_autocheckpoint=1000;
--   PRAGMA foreign_keys=ON;
--
-- At startup, run PRAGMA integrity_check; exit if result != 'ok'.

CREATE TABLE IF NOT EXISTS journal (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    reader_ip    TEXT NOT NULL,
    stream_epoch INTEGER NOT NULL,
    seq          INTEGER NOT NULL,
    reader_timestamp TEXT NOT NULL,
    raw_read_line TEXT NOT NULL,
    read_type    TEXT NOT NULL,
    acked        INTEGER NOT NULL DEFAULT 0,  -- 0=unacked, 1=acked
    received_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(reader_ip, stream_epoch, seq)
);

CREATE TABLE IF NOT EXISTS config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
