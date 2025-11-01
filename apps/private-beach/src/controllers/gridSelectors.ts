import { useMemo } from 'react';
import { defaultTileViewState, type TileViewState } from './gridViewState';
import type { GridDashboardMetadata } from './gridLayout';
import { useTileSnapshot } from './sessionTileController';

const FALLBACK_VIEW_STATE: TileViewState = defaultTileViewState();

export function selectTileViewState(grid: GridDashboardMetadata | null | undefined): TileViewState {
  if (!grid) {
    return FALLBACK_VIEW_STATE;
  }
  return grid.viewState ?? FALLBACK_VIEW_STATE;
}

export function useTileViewState(tileId: string): TileViewState {
  const snapshot = useTileSnapshot(tileId);
  return useMemo(() => selectTileViewState(snapshot.grid), [snapshot.grid]);
}
