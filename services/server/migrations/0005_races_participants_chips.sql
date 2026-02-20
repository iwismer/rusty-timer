-- Race management tables for participant and chip data imports.
CREATE TABLE races (
    race_id    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE participants (
    race_id     UUID NOT NULL REFERENCES races(race_id) ON DELETE CASCADE,
    bib         INTEGER NOT NULL,
    first_name  TEXT NOT NULL,
    last_name   TEXT NOT NULL,
    gender      TEXT NOT NULL DEFAULT 'X' CHECK (gender IN ('M', 'F', 'X')),
    affiliation TEXT,
    PRIMARY KEY (race_id, bib)
);

CREATE TABLE chips (
    race_id  UUID NOT NULL REFERENCES races(race_id) ON DELETE CASCADE,
    chip_id  TEXT NOT NULL,
    bib      INTEGER NOT NULL,
    PRIMARY KEY (race_id, chip_id)
);
