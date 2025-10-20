import type { NextApiRequest, NextApiResponse } from 'next';
import { eq } from 'drizzle-orm';
import { db, ensureMigrated } from '../../../db/client';
import { tileLayouts, tileLayoutPresets } from '../../../db/schema';
import type { BeachLayout } from '../../../lib/api';

const DEFAULT_LAYOUT: BeachLayout = {
  preset: 'grid2x2',
  tiles: [],
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
      })
      .from(tileLayouts)
      .where(eq(tileLayouts.privateBeachId, id))
      .limit(1);

    res.status(200).json(row ? ({ preset: row.preset, tiles: row.tiles ?? [] }) : DEFAULT_LAYOUT);
    return;
  }

  if (req.method === 'PUT') {
    const preset = typeof req.body?.preset === 'string' ? req.body.preset : '';
    if (!isValidPreset(preset)) {
      res.status(400).json({ error: 'Invalid preset' });
      return;
    }
    const tiles = normalizeTiles(req.body?.tiles);
    const now = new Date();

    await db
      .insert(tileLayouts)
      .values({
        privateBeachId: id,
        preset,
        tiles,
        updatedAt: now,
      })
      .onConflictDoUpdate({
        target: tileLayouts.privateBeachId,
        set: {
          preset,
          tiles,
          updatedAt: now,
        },
      });

    res.status(200).json({ preset, tiles });
    return;
  }

  res.setHeader('Allow', ['GET', 'PUT']);
  res.status(405).json({ error: 'Method not allowed' });
}
