CREATE TABLE stream_epoch_metadata (
    stream_id UUID NOT NULL REFERENCES streams(stream_id) ON DELETE CASCADE,
    stream_epoch BIGINT NOT NULL,
    name TEXT,
    PRIMARY KEY (stream_id, stream_epoch)
);
