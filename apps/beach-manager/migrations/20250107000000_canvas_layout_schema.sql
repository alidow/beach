CREATE TABLE IF NOT EXISTS surfer_canvas_layout (
    private_beach_id UUID PRIMARY KEY REFERENCES private_beach(id) ON DELETE CASCADE,
    layout JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

ALTER TABLE surfer_canvas_layout ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS surfer_canvas_layout_all ON surfer_canvas_layout;
CREATE POLICY surfer_canvas_layout_all ON surfer_canvas_layout
USING (
    private_beach_id::text = current_setting('beach.private_beach_id', true)
)
WITH CHECK (
    private_beach_id::text = current_setting('beach.private_beach_id', true)
);
