'use client';

import { useLayoutEffect, useMemo, useRef } from 'react';
import { buildManagerUrl } from '@/hooks/useManagerToken';
import { SURFACE_MIN_HEIGHT, SURFACE_MIN_WIDTH } from '../constants';
import { useTileState } from '../store';
import { deriveSurfaceBounds } from '../utils';
import { TileNode } from './TileNode';
import type { TileMovePayload } from '@/features/canvas/types';

type TileCanvasProps = {
  privateBeachId: string;
  managerUrl?: string;
  rewriteEnabled?: boolean;
  onTileMove?: (payload: TileMovePayload) => void;
};

export function TileCanvas({
  privateBeachId,
  managerUrl,
  rewriteEnabled = false,
  onTileMove,
}: TileCanvasProps) {
  const state = useTileState();
  const scrollRef = useRef<HTMLDivElement>(null);
  const bounds = useMemo(() => deriveSurfaceBounds(state), [state]);
  const isResizing = useMemo(() => Object.values(state.resizing).some(Boolean), [state.resizing]);
  const resolvedManagerUrl = buildManagerUrl(managerUrl);

  useLayoutEffect(() => {
    if (!isResizing) {
      return;
    }
    const node = scrollRef.current;
    if (!node) {
      return;
    }
    const { scrollLeft, scrollTop } = node;
    return () => {
      node.scrollLeft = scrollLeft;
      node.scrollTop = scrollTop;
    };
  }, [bounds.width, bounds.height, isResizing]);

  return (
    <div
      ref={scrollRef}
      className="tile-canvas"
      style={{ minHeight: SURFACE_MIN_HEIGHT, minWidth: SURFACE_MIN_WIDTH }}
    >
      <div
        className="tile-canvas__surface"
        style={{
          minWidth: bounds.width,
          minHeight: bounds.height,
        }}
      >
        {state.order.map((tileId, index) => {
          const tile = state.tiles[tileId];
          if (!tile) {
            return null;
          }
          return (
            <TileNode
              key={tile.id}
              tile={tile}
              orderIndex={index}
              isActive={state.activeId === tile.id}
              isResizing={Boolean(state.resizing[tile.id])}
              privateBeachId={privateBeachId}
              managerUrl={resolvedManagerUrl}
              rewriteEnabled={rewriteEnabled}
              onMove={onTileMove}
            />
          );
        })}
      </div>
    </div>
  );
}
