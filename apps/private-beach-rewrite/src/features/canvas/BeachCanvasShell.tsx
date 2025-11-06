'use client';

import Link from 'next/link';
import { useCallback, useEffect, useMemo } from 'react';
import { CanvasWorkspace } from './CanvasWorkspace';
import type { CanvasNodeDefinition, NodePlacementPayload, TileMovePayload } from './types';
import type { CanvasLayout } from '@/lib/api';
import { TileStoreProvider, layoutToTileState, serializeTileStateKey, useTileActions } from '@/features/tiles';
import { ManagerTokenProvider } from '@/hooks/ManagerTokenContext';
import { emitTelemetry } from '../../../../private-beach/src/lib/telemetry';
import { useTileLayoutPersistence } from './useTileLayoutPersistence';

type BeachCanvasShellProps = {
  beachId: string;
  beachName: string;
  backHref?: string;
  managerUrl?: string;
  managerToken?: string | null;
  initialLayout?: CanvasLayout | null;
  rewriteEnabled?: boolean;
  className?: string;
};

type BeachCanvasShellInnerProps = Omit<BeachCanvasShellProps, 'managerToken'> & {
  initialTileSignature?: string;
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
  beachName,
  backHref = '/beaches',
  managerUrl,
  managerToken,
  initialLayout,
  rewriteEnabled = false,
  className,
}: BeachCanvasShellProps) {
  const initialTileState = useMemo(() => layoutToTileState(initialLayout), [initialLayout]);
  const initialTileSignature = useMemo(
    () => serializeTileStateKey(initialTileState),
    [initialTileState],
  );

  return (
    <TileStoreProvider initialState={initialTileState}>
      <ManagerTokenProvider initialToken={managerToken}>
        <BeachCanvasShellInner
          beachId={beachId}
          beachName={beachName}
          backHref={backHref}
          managerUrl={managerUrl}
          initialLayout={initialLayout}
          initialTileSignature={initialTileSignature}
          rewriteEnabled={rewriteEnabled}
          className={className}
        />
      </ManagerTokenProvider>
    </TileStoreProvider>
  );
}

function BeachCanvasShellInner({
  beachId,
  beachName,
  backHref = '/beaches',
  managerUrl,
  initialLayout,
  initialTileSignature,
  rewriteEnabled = false,
  className,
}: BeachCanvasShellInnerProps) {
  const NAV_HEIGHT = 56;
  const CANVAS_VIEWPORT_HEIGHT = `calc(100vh - ${NAV_HEIGHT}px)`;
  const { createTile } = useTileActions();

  useTileLayoutPersistence({
    beachId,
    managerUrl,
    initialLayout,
    initialSignature: initialTileSignature,
  });

  const catalog = useMemo(() => DEFAULT_CATALOG, []);

  const handlePlacement = useCallback(
    (payload: NodePlacementPayload) => {
      const timestamp = Date.now();
      const recordId = buildPlacementId(payload, timestamp);

      createTile({
        id: recordId,
        nodeType: payload.nodeType as 'application',
        position: payload.snappedPosition,
        size: payload.size,
        focus: true,
      });

      console.info('[ws-c] node placed', { beachId, ...payload });
      emitTelemetry('canvas.tile.create', {
        privateBeachId: beachId,
        tileId: recordId,
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

  const wrapperClassName = ['relative flex h-full flex-1 min-h-0 w-full flex-col overflow-hidden', className ?? '']
    .filter(Boolean)
    .join(' ');

  return (
    <div className={wrapperClassName}>
      <header className="flex h-14 flex-shrink-0 items-center justify-between border-b border-border bg-background/80 px-4 backdrop-blur">
        <div className="flex items-center gap-3">
          {backHref ? (
            <Link
              href={backHref}
              className="inline-flex items-center gap-1 text-xs font-medium text-muted-foreground transition hover:text-foreground"
            >
              <span aria-hidden>←</span>
              Back
            </Link>
          ) : null}
          <div className="flex flex-col">
            <span className="text-sm font-semibold text-foreground">{beachName}</span>
          </div>
        </div>
      </header>
      <div
        className="relative flex flex-1 min-h-0 flex-col overflow-hidden"
        style={{ minHeight: CANVAS_VIEWPORT_HEIGHT }}
      >
        <CanvasWorkspace
          nodes={catalog}
          onNodePlacement={handlePlacement}
          onTileMove={handleTileMove}
          privateBeachId={beachId}
          managerUrl={managerUrl}
          rewriteEnabled={rewriteEnabled}
          initialDrawerOpen={false}
        />
      </div>
    </div>
  );
}
