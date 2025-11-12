'use client';

import { useEffect, useMemo, useState } from 'react';
import { CanvasUIProvider, useCanvasUI } from './CanvasContext';
import { CanvasEventsProvider } from './CanvasEventsContext';
import { FlowCanvas } from './FlowCanvas';
import { NodeDrawer } from './NodeDrawer';
import type { CanvasNodeDefinition, NodePlacementPayload, TileMovePayload } from './types';
import type { CanvasViewportState } from '@/features/tiles/types';

type CanvasWorkspaceProps = {
  nodes: CanvasNodeDefinition[];
  onNodePlacement: (payload: NodePlacementPayload) => void;
  onTileMove?: (payload: TileMovePayload) => void;
  onViewportChange?: (viewport: CanvasViewportState) => void;
  privateBeachId: string;
  managerUrl?: string;
  roadUrl?: string;
  rewriteEnabled: boolean;
  initialDrawerOpen?: boolean;
  gridSize?: number;
};

const DEFAULT_GRID_SIZE = 8;

function CanvasHotkeyBinder() {
  const { toggleDrawer } = useCanvasUI();

  useEffect(() => {
    const isMac = typeof navigator !== 'undefined' ? /Mac|iPhone|iPod|iPad/i.test(navigator.platform) : true;
    const handler = (event: KeyboardEvent) => {
      const isMeta = isMac ? event.metaKey : event.ctrlKey;
      if (isMeta && (event.key === 'b' || event.key === 'B')) {
        event.preventDefault();
        toggleDrawer();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [toggleDrawer]);

  return null;
}

export function CanvasWorkspace({
  nodes,
  onNodePlacement,
  onTileMove,
  onViewportChange,
  privateBeachId,
  managerUrl,
  roadUrl,
  rewriteEnabled,
  initialDrawerOpen = true,
  gridSize = DEFAULT_GRID_SIZE,
}: CanvasWorkspaceProps) {
  const [activeCatalogId, setActiveCatalogId] = useState<string | null>(null);

  const eventsValue = useMemo(
    () => ({
      reportTileMove: () => {
        // The Flow canvas handles telemetry + store synchronization directly.
      },
    }),
    [],
  );

  return (
    <CanvasEventsProvider value={eventsValue}>
      <CanvasUIProvider initialDrawerOpen={initialDrawerOpen}>
        <CanvasHotkeyBinder />
        <div className="relative flex h-full min-h-0 w-full">
          <FlowCanvas
            onNodePlacement={onNodePlacement}
            onTileMove={onTileMove}
            onViewportChange={onViewportChange}
            privateBeachId={privateBeachId}
            managerUrl={managerUrl}
            roadUrl={roadUrl}
            rewriteEnabled={rewriteEnabled}
            gridSize={gridSize}
          />
          <NodeDrawer
            nodes={nodes}
            activeNodeId={activeCatalogId}
            onNodeDragStart={setActiveCatalogId}
            onNodeDragEnd={() => setActiveCatalogId(null)}
          />
        </div>
      </CanvasUIProvider>
    </CanvasEventsProvider>
  );
}
