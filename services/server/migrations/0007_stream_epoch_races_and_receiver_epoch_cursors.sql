-- Rollback (manual SQL to reverse this migration, run in order):
--   1. ALTER TABLE receiver_cursors DROP CONSTRAINT IF EXISTS receiver_cursors_pkey;
--   2. ALTER TABLE receiver_cursors ADD PRIMARY KEY (receiver_id, stream_id);
--   3. DROP INDEX IF EXISTS idx_stream_epoch_races_race_id;
--   4. DROP TABLE IF EXISTS stream_epoch_races;

CREATE TABLE IF NOT EXISTS stream_epoch_races (
    stream_id UUID NOT NULL REFERENCES streams(stream_id) ON DELETE CASCADE,
    stream_epoch BIGINT NOT NULL,
    race_id UUID NOT NULL REFERENCES races(race_id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (stream_id, stream_epoch)
);

CREATE INDEX IF NOT EXISTS idx_stream_epoch_races_race_id
    ON stream_epoch_races (race_id);

-- Drop old two-column PK on receiver_cursors (idempotent: no-op if already removed)
DO $$ BEGIN
    ALTER TABLE receiver_cursors DROP CONSTRAINT receiver_cursors_pkey;
EXCEPTION WHEN undefined_object THEN
    NULL;
END $$;

-- Add new three-column PK on receiver_cursors (idempotent: no-op if already present)
DO $$ BEGIN
    ALTER TABLE receiver_cursors ADD PRIMARY KEY (receiver_id, stream_id, stream_epoch);
EXCEPTION WHEN duplicate_table THEN
    NULL;
END $$;
