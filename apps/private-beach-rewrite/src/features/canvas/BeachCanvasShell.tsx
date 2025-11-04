'use client';

import { useCallback, useEffect, useMemo, useState } from 'react';
import { CanvasWorkspace } from './CanvasWorkspace';
import type { CanvasNodeDefinition, NodePlacementPayload, TileMovePayload } from './types';
import { TileCanvas, TileStoreProvider, useTileActions, useTileState } from '@/features/tiles';
import { ManagerTokenProvider } from '@/hooks/ManagerTokenContext';
import { emitTelemetry } from '../../../../private-beach/src/lib/telemetry';

type BeachCanvasShellProps = {
  beachId: string;
  managerUrl?: string;
  managerToken?: string | null;
  rewriteEnabled?: boolean;
  className?: string;
};

type PlacementRecord = NodePlacementPayload & {
  id: string;
  createdAt: number;
};

const DEFAULT_CATALOG: CanvasNodeDefinition[] = [
  {
    id: 'application',
    nodeType: 'application',
    label: 'Application Tile',
    description: 'Launch a terminal + preview pair connected to an active session.',
    defaultSize: {
      width: 448,
      height: 320,
    },
  },
];

function buildPlacementId(base: NodePlacementPayload, suffix: number): string {
  return `${base.catalogId}-${base.snappedPosition.x}-${base.snappedPosition.y}-${suffix}`;
}

export function BeachCanvasShell({
  beachId,
  managerUrl,
  managerToken,
  rewriteEnabled = false,
  className,
}: BeachCanvasShellProps) {
  return (
    <TileStoreProvider>
      <ManagerTokenProvider initialToken={managerToken}>
        <BeachCanvasShellInner
          beachId={beachId}
          managerUrl={managerUrl}
          rewriteEnabled={rewriteEnabled}
          className={className}
        />
      </ManagerTokenProvider>
    </TileStoreProvider>
  );
}

function BeachCanvasShellInner({ beachId, managerUrl, rewriteEnabled = false, className }: BeachCanvasShellProps) {
  const [records, setRecords] = useState<PlacementRecord[]>([]);
  const { createTile } = useTileActions();
  const tileState = useTileState();

  const catalog = useMemo(() => DEFAULT_CATALOG, []);

  const handlePlacement = useCallback(
    (payload: NodePlacementPayload) => {
      const timestamp = Date.now();
      const record: PlacementRecord = {
        ...payload,
        id: buildPlacementId(payload, timestamp),
        createdAt: timestamp,
      };
      setRecords((previous) => [...previous, record]);

      createTile({
        id: record.id,
        nodeType: payload.nodeType as 'application',
        position: payload.snappedPosition,
        size: payload.size,
        focus: true,
      });

      console.info('[ws-c] node placed', { beachId, ...payload });
      emitTelemetry('canvas.tile.create', {
        privateBeachId: beachId,
        tileId: record.id,
        nodeType: payload.nodeType,
        position: payload.snappedPosition,
        width: payload.size.width,
        height: payload.size.height,
        rewriteEnabled,
      });
    },
    [beachId, createTile, rewriteEnabled],
  );

  const handleTileMove = useCallback(
    (payload: TileMovePayload) => {
      setRecords((previous) =>
        previous.map((record) =>
          record.id === payload.tileId
            ? {
                ...record,
                rawPosition: payload.rawPosition,
                snappedPosition: payload.snappedPosition,
              }
            : record,
        ),
      );
      console.info('[ws-c] tile moved', { beachId, ...payload });
      emitTelemetry('canvas.tile.move', {
        privateBeachId: beachId,
        tileId: payload.tileId,
        x: payload.snappedPosition.x,
        y: payload.snappedPosition.y,
        deltaX: payload.delta.x,
        deltaY: payload.delta.y,
        rewriteEnabled,
      });
    },
    [beachId, rewriteEnabled],
  );

  useEffect(() => {
    emitTelemetry('canvas.rewrite.flag-state', {
      privateBeachId: beachId,
      enabled: rewriteEnabled,
    });
  }, [beachId, rewriteEnabled]);

  useEffect(() => {
    setRecords((previous) =>
      previous.map((record) => {
        const tile = tileState.tiles[record.id];
        if (!tile) {
          return record;
        }
        if (
          tile.position.x === record.snappedPosition.x &&
          tile.position.y === record.snappedPosition.y &&
          tile.size.width === record.size.width &&
          tile.size.height === record.size.height
        ) {
          return record;
        }
        return {
          ...record,
          snappedPosition: { ...tile.position },
          size: { ...tile.size },
        };
      }),
    );
  }, [tileState.tiles]);

  const wrapperClassName = ['flex flex-col gap-4', className ?? ''].filter(Boolean).join(' ');

  return (
    <div className={wrapperClassName}>
      <CanvasWorkspace
        nodes={catalog}
        onNodePlacement={handlePlacement}
        onTileMove={handleTileMove}
        initialDrawerOpen
      >
        <TileCanvas
          privateBeachId={beachId}
          managerUrl={managerUrl}
          rewriteEnabled={rewriteEnabled}
        />
      </CanvasWorkspace>
      <section className="rounded-xl border border-border bg-card/70 p-4 shadow-inner">
        <header className="flex items-center justify-between">
          <h2 className="text-sm font-semibold text-foreground">Tile placements</h2>
          <span className="text-xs text-muted-foreground">{tileState.order.length}</span>
        </header>
        {records.length === 0 ? (
          <p className="mt-3 text-xs text-muted-foreground">
            Drag an Application tile from the node drawer to populate the canvas.
          </p>
        ) : (
          <ol className="mt-3 space-y-2 text-xs text-muted-foreground">
            {[...records].reverse().slice(0, 6).map((record) => (
              <li
                key={record.id}
                className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2 rounded-lg border border-border/50 bg-background/60 px-3 py-2"
              >
                <div className="flex flex-col gap-1">
                  <span className="font-medium text-foreground">
                    {record.nodeType} · {record.catalogId}
                  </span>
                  <span className="font-mono text-[11px]">
                    raw({Math.round(record.rawPosition.x)}, {Math.round(record.rawPosition.y)}) →
                    snapped({record.snappedPosition.x}, {record.snappedPosition.y})
                  </span>
                </div>
                <span className="text-[10px] uppercase tracking-wide text-muted-foreground">
                  {new Date(record.createdAt).toLocaleTimeString()}
                </span>
              </li>
            ))}
          </ol>
        )}
      </section>
    </div>
  );
}
