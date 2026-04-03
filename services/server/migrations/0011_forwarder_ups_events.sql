CREATE TABLE forwarder_ups_events (
    id              BIGSERIAL PRIMARY KEY,
    forwarder_id    TEXT NOT NULL,
    event_type      TEXT NOT NULL CHECK (event_type IN ('power_lost', 'power_restored')),
    battery_percent SMALLINT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_ups_events_forwarder ON forwarder_ups_events(forwarder_id, created_at);
