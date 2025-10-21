import type { NextApiRequest, NextApiResponse } from 'next';
import { eq } from 'drizzle-orm';
import { db, ensureMigrated } from '../../../db/client';
import { tileLayouts, tileLayoutPresets } from '../../../db/schema';
import type { TileLayoutCoordinates } from '../../../db/schema';
import type { BeachLayout } from '../../../lib/api';

const DEFAULT_LAYOUT: BeachLayout = {
  preset: 'grid2x2',
  tiles: [],
  layout: [],
};

function isValidPreset(value: string): value is BeachLayout['preset'] {
  return (tileLayoutPresets as readonly string[]).includes(value);
}

function normalizeTiles(input: unknown): string[] {
  if (!Array.isArray(input)) return [];
  const seen = new Set<string>();
  const clean: string[] = [];
  for (const item of input) {
    if (typeof item !== 'string') continue;
    const trimmed = item.trim();
    if (!trimmed || seen.has(trimmed)) continue;
    seen.add(trimmed);
    clean.push(trimmed);
    if (clean.length >= 12) break;
  }
  return clean;
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function normalizeLayout(input: unknown): TileLayoutCoordinates[] {
  if (!Array.isArray(input)) return [];
  const seen = new Set<string>();
  const clean: TileLayoutCoordinates[] = [];
  for (const raw of input) {
    if (!raw || typeof raw !== 'object') continue;
    const id = typeof (raw as any).id === 'string' ? (raw as any).id.trim() : '';
    if (!id || seen.has(id)) continue;
    const x = isFiniteNumber((raw as any).x) ? Math.max(0, Math.floor((raw as any).x)) : null;
    const y = isFiniteNumber((raw as any).y) ? Math.max(0, Math.floor((raw as any).y)) : null;
    const w = isFiniteNumber((raw as any).w) ? Math.max(1, Math.floor((raw as any).w)) : null;
    const h = isFiniteNumber((raw as any).h) ? Math.max(1, Math.floor((raw as any).h)) : null;
    if (x === null || y === null || w === null || h === null) continue;
    clean.push({ id, x, y, w, h });
    seen.add(id);
    if (clean.length >= 12) break;
  }
  return clean;
}

export default async function handler(req: NextApiRequest, res: NextApiResponse) {
  const { id } = req.query;
  if (typeof id !== 'string' || id.length === 0) {
    res.status(400).json({ error: 'Invalid beach id' });
    return;
  }

  await ensureMigrated();

  if (req.method === 'GET') {
    const [row] = await db
      .select({
        preset: tileLayouts.preset,
        tiles: tileLayouts.tiles,
        layout: tileLayouts.layout,
      })
      .from(tileLayouts)
      .where(eq(tileLayouts.privateBeachId, id))
      .limit(1);

    if (!row) {
      res.status(200).json(DEFAULT_LAYOUT);
      return;
    }

    res.status(200).json({
      preset: row.preset,
      tiles: normalizeTiles(row.tiles),
      layout: normalizeLayout(row.layout),
    });
    return;
  }

  if (req.method === 'PUT') {
    const preset = typeof req.body?.preset === 'string' ? req.body.preset : '';
    if (!isValidPreset(preset)) {
      res.status(400).json({ error: 'Invalid preset' });
      return;
    }
    const tiles = normalizeTiles(req.body?.tiles);
    const layout = normalizeLayout(req.body?.layout).filter((item) => tiles.includes(item.id));
    const now = new Date();

    await db
      .insert(tileLayouts)
      .values({
        privateBeachId: id,
        preset,
        tiles,
        layout,
        updatedAt: now,
      })
      .onConflictDoUpdate({
        target: tileLayouts.privateBeachId,
        set: {
          preset,
          tiles,
          layout,
          updatedAt: now,
        },
      });

    res.status(200).json({ preset, tiles, layout });
    return;
  }

  res.setHeader('Allow', ['GET', 'PUT']);
  res.status(405).json({ error: 'Method not allowed' });
}
