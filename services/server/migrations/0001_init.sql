-- Remote Forwarding Suite: Initial PostgreSQL Schema
-- Migration: 0001_init.sql
--
-- This migration creates the core tables for the remote forwarding server.
-- See docs/plans/2026-02-17-remote-forwarding-design/03-server-design.md
-- for the authoritative schema definition.

-- pgcrypto provides gen_random_uuid() on Postgres < 13.
-- On Postgres 13+ it is a built-in; IF NOT EXISTS makes this a no-op.
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- device_tokens
-- token_hash stores SHA-256(raw_token_bytes) — always exactly 32 bytes.
-- The server never stores raw bearer tokens.
CREATE TABLE device_tokens (
    token_id     UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash   BYTEA NOT NULL UNIQUE,
    device_type  TEXT NOT NULL CHECK (device_type IN ('forwarder', 'receiver')),
    device_id    TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at   TIMESTAMPTZ
);

-- streams
CREATE TABLE streams (
    stream_id      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    forwarder_id   TEXT NOT NULL,
    reader_ip      TEXT NOT NULL,
    display_alias  TEXT,
    stream_epoch   BIGINT NOT NULL DEFAULT 1,
    online         BOOLEAN NOT NULL DEFAULT false,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (forwarder_id, reader_ip)
);

-- events
CREATE TABLE events (
    stream_id        UUID NOT NULL REFERENCES streams(stream_id),
    stream_epoch     BIGINT NOT NULL,
    seq              BIGINT NOT NULL,
    reader_timestamp TEXT,
    raw_frame       BYTEA NOT NULL,
    read_type        TEXT NOT NULL,
    received_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (stream_id, stream_epoch, seq)
);

-- stream_metrics
CREATE TABLE stream_metrics (
    stream_id                        UUID PRIMARY KEY REFERENCES streams(stream_id),
    raw_count                        BIGINT NOT NULL DEFAULT 0,
    dedup_count                      BIGINT NOT NULL DEFAULT 0,
    retransmit_count                 BIGINT NOT NULL DEFAULT 0,
    last_canonical_event_received_at TIMESTAMPTZ
);

-- receiver_cursors
CREATE TABLE receiver_cursors (
    receiver_id  TEXT NOT NULL,
    stream_id    UUID NOT NULL REFERENCES streams(stream_id),
    stream_epoch BIGINT NOT NULL,
    last_seq     BIGINT NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (receiver_id, stream_id)
);
