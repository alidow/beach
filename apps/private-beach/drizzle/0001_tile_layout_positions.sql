ALTER TABLE surfer_tile_layout
ADD COLUMN IF NOT EXISTS layout JSONB NOT NULL DEFAULT '[]'::jsonb;
