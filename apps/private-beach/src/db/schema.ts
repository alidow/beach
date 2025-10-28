import { sql } from 'drizzle-orm';
import { jsonb, pgTable, text, timestamp } from 'drizzle-orm/pg-core';

export const tileLayoutPresets = ['grid2x2', 'onePlusThree', 'focus'] as const;

export type TileLayoutCoordinates = {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  widthPx?: number | null;
  heightPx?: number | null;
  zoom?: number | null;
  locked?: boolean | null;
  toolbarPinned?: boolean | null;
};

export const tileLayouts = pgTable('surfer_tile_layout', {
  privateBeachId: text('private_beach_id').primaryKey(),
  preset: text('preset')
    .notNull()
    .$type<(typeof tileLayoutPresets)[number]>(),
  tiles: text('tiles')
    .array()
    .notNull()
    .default(sql`ARRAY[]::text[]`),
  layout: jsonb('layout')
    .$type<TileLayoutCoordinates[]>()
    .notNull()
    .default(sql`'[]'::jsonb`),
  updatedAt: timestamp('updated_at', { withTimezone: true })
    .notNull()
    .default(sql`now()`),
});

export type TileLayout = typeof tileLayouts.$inferSelect;
export type NewTileLayout = typeof tileLayouts.$inferInsert;
