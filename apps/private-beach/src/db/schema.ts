import { sql } from 'drizzle-orm';
import { pgTable, text, timestamp } from 'drizzle-orm/pg-core';

export const tileLayoutPresets = ['grid2x2', 'onePlusThree', 'focus'] as const;

export const tileLayouts = pgTable('surfer_tile_layout', {
  privateBeachId: text('private_beach_id').primaryKey(),
  preset: text('preset')
    .notNull()
    .$type<(typeof tileLayoutPresets)[number]>(),
  tiles: text('tiles')
    .array()
    .notNull()
    .default(sql`ARRAY[]::text[]`),
  updatedAt: timestamp('updated_at', { withTimezone: true })
    .notNull()
    .default(sql`now()`),
});

export type TileLayout = typeof tileLayouts.$inferSelect;
export type NewTileLayout = typeof tileLayouts.$inferInsert;
