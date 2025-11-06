'use client';

import Link from 'next/link';
import { useCallback, useEffect, useMemo } from 'react';
import { CanvasWorkspace } from './CanvasWorkspace';
import type { CanvasNodeDefinition, NodePlacementPayload, TileMovePayload } from './types';
import { TileStoreProvider, useTileActions } from '@/features/tiles';
import { ManagerTokenProvider } from '@/hooks/ManagerTokenContext';
import { emitTelemetry } from '../../../../private-beach/src/lib/telemetry';

type BeachCanvasShellProps = {
  beachId: string;
  beachName: string;
  backHref?: string;
  managerUrl?: string;
  managerToken?: string | null;
  rewriteEnabled?: boolean;
  className?: string;
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
  rewriteEnabled = false,
  className,
}: BeachCanvasShellProps) {
  return (
    <TileStoreProvider>
      <ManagerTokenProvider initialToken={managerToken}>
        <BeachCanvasShellInner
          beachId={beachId}
          beachName={beachName}
          backHref={backHref}
          managerUrl={managerUrl}
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
  rewriteEnabled = false,
  className,
}: BeachCanvasShellProps) {
  const { createTile } = useTileActions();

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

  const wrapperClassName = [
    'relative z-0 flex h-full min-h-0 w-full flex-col bg-slate-950 text-slate-200',
    "after:pointer-events-none after:absolute after:inset-0 after:-z-10 after:content-[''] after:bg-[radial-gradient(circle_at_top,rgba(56,189,248,0.18),transparent_60%)]",
    className ?? '',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <div className={wrapperClassName}>
      <header className="relative z-30 flex h-12 items-center justify-between border-b border-white/10 bg-slate-950/70 px-6 backdrop-blur-xl">
        <div className="flex items-center gap-4">
          {backHref ? (
            <Link
              href={backHref}
              className="inline-flex h-8 w-8 items-center justify-center rounded-full border border-white/10 bg-white/5 text-[13px] font-semibold text-slate-300 transition hover:text-white"
            >
              <span aria-hidden>←</span>
            </Link>
          ) : null}
          <div className="flex items-center gap-3">
            <span className="text-sm font-semibold text-white/90">{beachName}</span>
          </div>
        </div>
      </header>
      <div className="relative flex flex-1 min-h-0 flex-col overflow-hidden">
        <CanvasWorkspace
          nodes={catalog}
          onNodePlacement={handlePlacement}
          onTileMove={handleTileMove}
          privateBeachId={beachId}
          managerUrl={managerUrl}
          rewriteEnabled={rewriteEnabled}
          initialDrawerOpen
        />
      </div>
    </div>
  );
}
