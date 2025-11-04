'use client';

import {
  DEFAULT_TILE_HEIGHT,
  DEFAULT_TILE_WIDTH,
  MIN_TILE_HEIGHT,
  MIN_TILE_WIDTH,
  SURFACE_MIN_HEIGHT,
  SURFACE_MIN_WIDTH,
  SURFACE_PADDING_PX,
  TILE_GAP_PX,
  TILE_GRID_SNAP_PX,
} from './constants';
import type { TilePosition, TileSize, TileState } from './types';

function snap(value: number): number {
  if (!Number.isFinite(value)) {
    return 0;
  }
  return Math.round(value / TILE_GRID_SNAP_PX) * TILE_GRID_SNAP_PX;
}

export function snapSize(size: Partial<TileSize> | undefined): TileSize {
  const width = Math.max(MIN_TILE_WIDTH, snap(size?.width ?? DEFAULT_TILE_WIDTH));
  const height = Math.max(MIN_TILE_HEIGHT, snap(size?.height ?? DEFAULT_TILE_HEIGHT));
  return { width, height };
}

export function snapPosition(position: Partial<TilePosition> | undefined): TilePosition {
  const x = snap(position?.x ?? 0);
  const y = snap(position?.y ?? 0);
  return { x, y };
}

export function computeAutoPosition(state: TileState, size: TileSize): TilePosition {
  const count = state.order.length;
  const estimatedColumns = Math.max(2, Math.floor(SURFACE_MIN_WIDTH / (size.width + TILE_GAP_PX)) || 2);
  const column = count % estimatedColumns;
  const row = Math.floor(count / estimatedColumns);
  const x = snap(column * (size.width + TILE_GAP_PX));
  const y = snap(row * (size.height + TILE_GAP_PX));
  return { x, y };
}

export function generateTileId(state: TileState, preferred?: string): string {
  if (preferred && !state.tiles[preferred]) {
    return preferred;
  }
  if (preferred && state.tiles[preferred]) {
    return preferred;
  }
  const existing = new Set(state.order);
  let attempt = 0;
  while (attempt < 1000) {
    const candidate =
      typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
        ? crypto.randomUUID()
        : `tile-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
    if (!existing.has(candidate)) {
      return candidate;
    }
    attempt += 1;
  }
  return `tile-${state.order.length + 1}`;
}

export function deriveSurfaceBounds(state: TileState): { width: number; height: number } {
  let maxRight = SURFACE_MIN_WIDTH - SURFACE_PADDING_PX;
  let maxBottom = SURFACE_MIN_HEIGHT - SURFACE_PADDING_PX;
  for (const tileId of state.order) {
    const tile = state.tiles[tileId];
    if (!tile) continue;
    const right = tile.position.x + tile.size.width;
    const bottom = tile.position.y + tile.size.height;
    if (right > maxRight) {
      maxRight = right;
    }
    if (bottom > maxBottom) {
      maxBottom = bottom;
    }
  }
  const width = Math.max(SURFACE_MIN_WIDTH, snap(maxRight + SURFACE_PADDING_PX));
  const height = Math.max(SURFACE_MIN_HEIGHT, snap(maxBottom + SURFACE_PADDING_PX));
  return { width, height };
}
