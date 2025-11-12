import { sql } from 'drizzle-orm';
import { jsonb, pgTable, text, timestamp } from 'drizzle-orm/pg-core';
import type { AgentRelationshipCadence, AgentRelationshipUpdateMode } from '../lib/api';

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

// Canvas layout (v3) persisted as a JSON graph per beach.
export type CanvasAgentRelationshipJson = {
  id: string;
  sourceId: string;
  targetId: string;
  sourceSessionId?: string | null;
  targetSessionId?: string | null;
  sourceHandleId?: string | null;
  targetHandleId?: string | null;
  instructions?: string | null;
  updateMode?: AgentRelationshipUpdateMode;
  pollFrequency?: number | null;
  cadence?: AgentRelationshipCadence;
};

export type CanvasLayoutJson = {
  version: 3;
  viewport: { zoom: number; pan: { x: number; y: number } };
  tiles: Record<
    string,
    {
      id: string;
      kind: 'application';
      position: { x: number; y: number };
      size: { width: number; height: number };
      zIndex: number;
      groupId?: string;
      zoom?: number;
      locked?: boolean;
      toolbarPinned?: boolean;
    }
  >;
  agents: Record<
    string,
    {
      id: string;
      position: { x: number; y: number };
      size: { width: number; height: number };
      zIndex: number;
      icon?: string;
      status?: 'idle' | 'controlling';
    }
  >;
  groups: Record<
    string,
    {
      id: string;
      name?: string;
      memberIds: string[];
      position: { x: number; y: number };
      size: { width: number; height: number };
      zIndex: number;
      collapsed?: boolean;
    }
  >;
  controlAssignments: Record<string, { controllerId: string; targetType: 'tile' | 'group'; targetId: string }>;
  metadata: {
    createdAt: number;
    updatedAt: number;
    migratedFrom?: number;
    agentRelationships?: Record<string, CanvasAgentRelationshipJson>;
    agentRelationshipOrder?: string[];
  };
};

export const canvasLayouts = pgTable('surfer_canvas_layout', {
  privateBeachId: text('private_beach_id').primaryKey(),
  layout: jsonb('layout').$type<CanvasLayoutJson>().notNull().default(sql`'{}'::jsonb`),
  updatedAt: timestamp('updated_at', { withTimezone: true })
    .notNull()
    .default(sql`now()`),
});

export type CanvasLayoutRow = typeof canvasLayouts.$inferSelect;
export type NewCanvasLayoutRow = typeof canvasLayouts.$inferInsert;
