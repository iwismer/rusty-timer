CREATE TABLE stream_epoch_races (
    stream_id UUID NOT NULL REFERENCES streams(stream_id) ON DELETE CASCADE,
    stream_epoch BIGINT NOT NULL,
    race_id UUID NOT NULL REFERENCES races(race_id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (stream_id, stream_epoch)
);

CREATE INDEX idx_stream_epoch_races_race_id
    ON stream_epoch_races (race_id);

ALTER TABLE receiver_cursors
    DROP CONSTRAINT receiver_cursors_pkey;

ALTER TABLE receiver_cursors
    ADD PRIMARY KEY (receiver_id, stream_id, stream_epoch);
