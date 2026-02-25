-- Receiver local SQLite schema (v1)
-- Authority: 04-receiver-design.md
-- Required PRAGMAs (set at connection open, not in this file):
--   PRAGMA journal_mode=WAL;
--   PRAGMA synchronous=FULL;
--   PRAGMA wal_autocheckpoint=1000;
--   PRAGMA foreign_keys=ON;
-- At startup, run PRAGMA integrity_check; exit if result != 'ok'.

CREATE TABLE IF NOT EXISTS profile (
    server_url  TEXT NOT NULL,
    token       TEXT NOT NULL,
    update_mode TEXT NOT NULL DEFAULT 'check-and-download',
    selection_json TEXT NOT NULL DEFAULT '{"mode":"manual","streams":[]}',
    replay_policy TEXT NOT NULL DEFAULT 'resume',
    replay_targets_json TEXT,
    receiver_mode_json TEXT
);

CREATE TABLE IF NOT EXISTS subscriptions (
    forwarder_id       TEXT NOT NULL,
    reader_ip          TEXT NOT NULL,
    local_port_override INTEGER,
    PRIMARY KEY (forwarder_id, reader_ip)
);

CREATE TABLE IF NOT EXISTS cursors (
    forwarder_id      TEXT NOT NULL,
    reader_ip         TEXT NOT NULL,
    stream_epoch      BIGINT NOT NULL,
    acked_through_seq BIGINT NOT NULL,
    PRIMARY KEY (forwarder_id, reader_ip)
);

CREATE TABLE IF NOT EXISTS earliest_epochs (
    forwarder_id   TEXT NOT NULL,
    reader_ip      TEXT NOT NULL,
    earliest_epoch BIGINT NOT NULL,
    PRIMARY KEY (forwarder_id, reader_ip)
);
