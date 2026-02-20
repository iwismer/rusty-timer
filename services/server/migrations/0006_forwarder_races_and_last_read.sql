-- Forwarder-to-race assignment (many forwarders : one race)
CREATE TABLE forwarder_races (
    forwarder_id TEXT PRIMARY KEY,
    race_id UUID REFERENCES races(race_id) ON DELETE SET NULL
);

-- Last-read tracking on stream_metrics (for overview display)
ALTER TABLE stream_metrics
  ADD COLUMN last_tag_id TEXT,
  ADD COLUMN last_reader_timestamp TEXT;
