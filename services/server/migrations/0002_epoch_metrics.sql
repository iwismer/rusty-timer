ALTER TABLE events ADD COLUMN tag_id TEXT;

ALTER TABLE stream_metrics
  ADD COLUMN epoch_raw_count BIGINT NOT NULL DEFAULT 0,
  ADD COLUMN epoch_dedup_count BIGINT NOT NULL DEFAULT 0,
  ADD COLUMN epoch_retransmit_count BIGINT NOT NULL DEFAULT 0,
  ADD COLUMN epoch_last_received_at TIMESTAMPTZ;
