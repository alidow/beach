CREATE TABLE IF NOT EXISTS surfer_tile_layout (
    private_beach_id TEXT PRIMARY KEY,
    preset TEXT NOT NULL,
    tiles TEXT[] NOT NULL DEFAULT '{}',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
