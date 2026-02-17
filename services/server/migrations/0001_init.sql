-- Remote Forwarding Suite: Initial PostgreSQL Schema
-- Migration: 0001_init.sql
--
-- This migration creates the core tables for the remote forwarding server.
-- See docs/plans/2026-02-17-remote-forwarding-design/01-common-requirements.md
-- for design rationale.

-- device_tokens
-- token_hash stores SHA-256(raw_token_bytes) — always exactly 32 bytes.
-- The server never stores raw bearer tokens.
CREATE TABLE device_tokens (
    token_hash  BYTEA PRIMARY KEY,  -- SHA-256(raw_token_bytes)
    device_id   TEXT NOT NULL,
    device_type TEXT NOT NULL,  -- 'forwarder' | 'receiver'
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at  TIMESTAMPTZ
);

-- streams
-- Immutable stream identity: (forwarder_id, reader_ip).
CREATE TABLE streams (
    id            BIGSERIAL PRIMARY KEY,
    forwarder_id  TEXT NOT NULL,
    reader_ip     TEXT NOT NULL,
    display_alias TEXT,
    stream_epoch  INTEGER NOT NULL DEFAULT 1,
    online        BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE(forwarder_id, reader_ip)
);

-- events
-- Event identity: (stream_id, stream_epoch, seq).
-- Identical-key retransmits are rejected (PK violation);
-- dedup/retransmit counters are maintained in stream_metrics.
CREATE TABLE events (
    stream_id        BIGINT NOT NULL REFERENCES streams(id),
    stream_epoch     INTEGER NOT NULL,
    seq              BIGINT NOT NULL,
    reader_timestamp TIMESTAMPTZ NOT NULL,
    raw_read_line    TEXT NOT NULL,  -- UTF-8 (ASCII IPICO payload expected)
    read_type        TEXT NOT NULL,  -- 'RAW' | 'FSLS'
    received_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (stream_id, stream_epoch, seq)
);

-- Reinforces PK; useful for dedup checks and explicit index scans.
CREATE UNIQUE INDEX events_identity ON events(stream_id, stream_epoch, seq);

-- stream_metrics
-- Lifetime totals per stream.
-- Invariant: raw_count = dedup_count + retransmit_count.
-- Lag definition: now - last_canonical_event_received_at (null when no events).
CREATE TABLE stream_metrics (
    stream_id                        BIGINT PRIMARY KEY REFERENCES streams(id),
    raw_count                        BIGINT NOT NULL DEFAULT 0,
    dedup_count                      BIGINT NOT NULL DEFAULT 0,
    retransmit_count                 BIGINT NOT NULL DEFAULT 0,
    last_canonical_event_received_at TIMESTAMPTZ
);

-- receiver_cursors
-- Tracks the last event each receiver has acknowledged per stream.
CREATE TABLE receiver_cursors (
    receiver_id  TEXT NOT NULL,
    stream_id    BIGINT NOT NULL REFERENCES streams(id),
    stream_epoch INTEGER NOT NULL,
    last_seq     BIGINT NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (receiver_id, stream_id)
);
