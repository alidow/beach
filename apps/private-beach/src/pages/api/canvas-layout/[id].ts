import type { NextApiRequest, NextApiResponse } from 'next';
import { eq } from 'drizzle-orm';
import { db, ensureMigrated } from '../../../db/client';
import { canvasLayouts, tileLayouts } from '../../../db/schema';
import type { AgentRelationshipCadence, AgentRelationshipUpdateMode } from '../../../lib/api';

type CanvasAgentRelationship = {
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

type CanvasLayout = {
	version: 3;
	viewport: { zoom: number; pan: { x: number; y: number } };
	tiles: Record<string, { id: string; kind: 'application'; position: { x: number; y: number }; size: { width: number; height: number }; zIndex: number; groupId?: string; zoom?: number; locked?: boolean; toolbarPinned?: boolean }>;
	agents: Record<string, { id: string; position: { x: number; y: number }; size: { width: number; height: number }; zIndex: number; icon?: string; status?: 'idle' | 'controlling' }>;
	groups: Record<string, { id: string; name?: string; memberIds: string[]; position: { x: number; y: number }; size: { width: number; height: number }; zIndex: number; collapsed?: boolean }>;
	controlAssignments: Record<string, { controllerId: string; targetType: 'tile' | 'group'; targetId: string }>;
	metadata: {
		createdAt: number;
		updatedAt: number;
		migratedFrom?: number;
		agentRelationships?: Record<string, CanvasAgentRelationship>;
		agentRelationshipOrder?: string[];
	};
};

function isRecord(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === 'object' && !Array.isArray(v);
}

function isCanvasLayout(input: unknown): input is CanvasLayout {
  if (!isRecord(input)) return false;
  if ((input as any).version !== 3) return false;
  const vp = (input as any).viewport;
  if (!isRecord(vp) || typeof (vp as any).zoom !== 'number' || !isRecord((vp as any).pan)) return false;
  if (typeof (vp as any).pan.x !== 'number' || typeof (vp as any).pan.y !== 'number') return false;
  const tiles = (input as any).tiles;
  const agents = (input as any).agents;
  const groups = (input as any).groups;
  const ctrl = (input as any).controlAssignments;
  const meta = (input as any).metadata;
  if (!isRecord(tiles) || !isRecord(agents) || !isRecord(groups) || !isRecord(ctrl) || !isRecord(meta)) return false;
  if (typeof (meta as any).createdAt !== 'number' || typeof (meta as any).updatedAt !== 'number') return false;
  return true;
}

const EMPTY: CanvasLayout = {
  version: 3,
  viewport: { zoom: 1, pan: { x: 0, y: 0 } },
  tiles: {},
  agents: {},
  groups: {},
  controlAssignments: {},
  metadata: { createdAt: Date.now(), updatedAt: Date.now() },
};

export default async function handler(req: NextApiRequest, res: NextApiResponse) {
  const { id } = req.query;
  if (typeof id !== 'string' || id.length === 0) {
    res.status(400).json({ error: 'Invalid beach id' });
    return;
  }

  await ensureMigrated();

  if (req.method === 'GET') {
    const [row] = await db
      .select({ layout: canvasLayouts.layout })
      .from(canvasLayouts)
      .where(eq(canvasLayouts.privateBeachId, id))
      .limit(1);
    if (row && isCanvasLayout(row.layout)) {
      res.status(200).json(row.layout);
      return;
    }
    // Attempt one-time best-effort migration from grid layout if present.
    const [grid] = await db
      .select({ preset: tileLayouts.preset, tiles: tileLayouts.tiles, layout: tileLayouts.layout })
      .from(tileLayouts)
      .where(eq(tileLayouts.privateBeachId, id))
      .limit(1);
    if (grid) {
      const now = Date.now();
      const migrated: CanvasLayout = {
        version: 3,
        viewport: { zoom: 1, pan: { x: 0, y: 0 } },
        tiles: {},
        agents: {},
        groups: {},
        controlAssignments: {},
        metadata: { createdAt: now, updatedAt: now, migratedFrom: 2 },
      };
      const items = Array.isArray(grid.layout) ? grid.layout : [];
      for (const item of items as any[]) {
        const idStr = typeof item?.id === 'string' ? item.id : undefined;
        if (!idStr) continue;
        const x = Number.isFinite(item.x) ? Math.max(0, Math.floor(item.x)) : 0;
        const y = Number.isFinite(item.y) ? Math.max(0, Math.floor(item.y)) : 0;
        const w = Number.isFinite(item.w) ? Math.max(1, Math.floor(item.w)) : 2;
        const h = Number.isFinite(item.h) ? Math.max(1, Math.floor(item.h)) : 2;
        const width = Number.isFinite(item.widthPx) ? Math.max(50, Math.round(item.widthPx)) : w * 320;
        const height = Number.isFinite(item.heightPx) ? Math.max(50, Math.round(item.heightPx)) : h * 240;
        migrated.tiles[idStr] = {
          id: idStr,
          kind: 'application',
          position: { x: x * 320, y: y * 240 },
          size: { width, height },
          zIndex: 1,
          zoom: typeof item.zoom === 'number' ? item.zoom : undefined,
          locked: typeof item.locked === 'boolean' ? item.locked : undefined,
          toolbarPinned: typeof item.toolbarPinned === 'boolean' ? item.toolbarPinned : undefined,
        };
      }
      // Persist migrated layout for future requests
      await db
        .insert(canvasLayouts)
        .values({ privateBeachId: id, layout: migrated, updatedAt: new Date() })
        .onConflictDoUpdate({ target: canvasLayouts.privateBeachId, set: { layout: migrated, updatedAt: new Date() } });
      res.status(200).json(migrated);
      return;
    }
    res.status(200).json(EMPTY);
    return;
  }

  if (req.method === 'PUT') {
    const body = req.body;
    if (!isCanvasLayout(body)) {
      res.status(400).json({ error: 'invalid canvas layout' });
      return;
    }
    const now = new Date();
    const layout: CanvasLayout = { ...body, metadata: { ...body.metadata, updatedAt: now.getTime() } };
    await db
      .insert(canvasLayouts)
      .values({ privateBeachId: id, layout, updatedAt: now })
      .onConflictDoUpdate({
        target: canvasLayouts.privateBeachId,
        set: { layout, updatedAt: now },
      });
    res.status(200).json(layout);
    return;
  }

  res.setHeader('Allow', ['GET', 'PUT']);
  res.status(405).json({ error: 'Method not allowed' });
}
