CREATE INDEX IF NOT EXISTS idx_events_stream_epoch_tag_id_not_null
ON events (stream_id, stream_epoch, tag_id)
WHERE tag_id IS NOT NULL;
