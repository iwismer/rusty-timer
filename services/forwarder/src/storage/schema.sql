-- Forwarder local SQLite schema (v1 Frozen)
-- Task 6 schema — supersedes the Task 4 stub.
--
-- Required PRAGMAs (set at connection open, not in this file):
--   PRAGMA journal_mode=WAL;
--   PRAGMA synchronous=FULL;
--   PRAGMA wal_autocheckpoint=1000;
--   PRAGMA foreign_keys=ON;
--
-- At startup, run PRAGMA integrity_check; exit if result != 'ok'.

CREATE TABLE IF NOT EXISTS journal (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    stream_key      TEXT NOT NULL,
    stream_epoch    BIGINT NOT NULL,
    seq             BIGINT NOT NULL,
    reader_timestamp TEXT,
    raw_frame       BLOB NOT NULL,
    read_type       TEXT NOT NULL,
    received_at     TEXT NOT NULL,
    UNIQUE(stream_key, stream_epoch, seq)
);

CREATE TABLE IF NOT EXISTS stream_state (
    stream_key          TEXT PRIMARY KEY,
    stream_epoch        BIGINT NOT NULL,
    next_seq            BIGINT NOT NULL,
    acked_epoch         BIGINT NOT NULL DEFAULT 0,
    acked_through_seq   BIGINT NOT NULL DEFAULT 0
);
