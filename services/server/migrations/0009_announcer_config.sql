CREATE TABLE announcer_config (
    id SMALLINT PRIMARY KEY CHECK (id = 1),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    enabled_until TIMESTAMPTZ,
    selected_stream_ids UUID[] NOT NULL DEFAULT '{}',
    max_list_size INTEGER NOT NULL DEFAULT 25,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO announcer_config (id) VALUES (1)
ON CONFLICT (id) DO NOTHING;
