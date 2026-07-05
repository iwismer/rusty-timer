CREATE TABLE IF NOT EXISTS streams (
    stream_id TEXT PRIMARY KEY,
    hardware_reader_id TEXT NOT NULL,
    network_addr TEXT NOT NULL,
    display_name TEXT NOT NULL,
    reader_connected INTEGER NOT NULL DEFAULT 0,
    created_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS stream_epochs (
    stream_id TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    start_seq INTEGER NOT NULL,
    end_seq INTEGER,
    reason TEXT NOT NULL,
    created_unix_ms INTEGER,
    PRIMARY KEY (stream_id, epoch),
    FOREIGN KEY (stream_id) REFERENCES streams(stream_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS events (
    stream_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    raw_frame BLOB NOT NULL,
    read_kind TEXT NOT NULL,
    reader_timestamp TEXT,
    received_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (stream_id, seq),
    FOREIGN KEY (stream_id) REFERENCES streams(stream_id) ON DELETE CASCADE,
    FOREIGN KEY (stream_id, epoch) REFERENCES stream_epochs(stream_id, epoch) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS receivers (
    endpoint_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    approved_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS receiver_stream_cursors (
    endpoint_id TEXT NOT NULL,
    stream_id TEXT NOT NULL,
    acked_through_seq INTEGER NOT NULL,
    PRIMARY KEY (endpoint_id, stream_id),
    FOREIGN KEY (endpoint_id) REFERENCES receivers(endpoint_id) ON DELETE CASCADE,
    FOREIGN KEY (stream_id) REFERENCES streams(stream_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS stream_retention (
    stream_id TEXT PRIMARY KEY,
    earliest_available_seq INTEGER NOT NULL,
    forced_gap_count INTEGER NOT NULL,
    FOREIGN KEY (stream_id) REFERENCES streams(stream_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_events_stream_epoch_seq ON events(stream_id, epoch, seq);

-- Supports retention pruning predicates that scan by age then narrow to a
-- stream's sequence range (received_unix_ms < cutoff, ordered by seq).
CREATE INDEX IF NOT EXISTS idx_events_received_stream_seq
    ON events(received_unix_ms, stream_id, seq);

-- Supports the per-stream MIN(acked_through_seq) lookups used to classify
-- acked vs. unacked events during pruning.
CREATE INDEX IF NOT EXISTS idx_cursors_stream_acked
    ON receiver_stream_cursors(stream_id, acked_through_seq);
