CREATE TABLE IF NOT EXISTS surfer_canvas_layout (
    private_beach_id TEXT PRIMARY KEY,
    layout JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
