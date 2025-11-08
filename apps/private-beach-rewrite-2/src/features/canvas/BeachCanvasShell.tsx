'use client';

import Link from 'next/link';
import { useCallback, useEffect, useMemo } from 'react';
import { CanvasWorkspace } from './CanvasWorkspace';
import type { CanvasNodeDefinition, NodePlacementPayload, TileMovePayload } from './types';
import type { CanvasLayout } from '@/lib/api';
import type { SessionSummary } from '@private-beach/shared-api';
import { TileStoreProvider, layoutToTileState, serializeTileStateKey, useTileActions, useTileState } from '@/features/tiles';
import type { TileNodeType } from '@/features/tiles/types';
import { extractTileLinkFromMetadata, sessionSummaryToTileMeta } from '@/features/tiles/sessionMeta';
import { ManagerTokenProvider } from '@/hooks/ManagerTokenContext';
import { buildManagerUrl } from '@/hooks/useManagerToken';
import { emitTelemetry } from '../../../../private-beach/src/lib/telemetry';
import { useTileLayoutPersistence } from './useTileLayoutPersistence';

type BeachCanvasShellProps = {
  beachId: string;
  beachName: string;
  backHref?: string;
  managerUrl?: string;
  managerToken?: string | null;
  initialLayout?: CanvasLayout | null;
  initialSessions?: SessionSummary[];
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
  {
    id: 'agent',
    nodeType: 'agent',
    label: 'Agent Tile',
    description: 'Describe an automation agent and connect it to applications to orchestrate work.',
    defaultSize: {
      width: 360,
      height: 260,
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
  initialSessions,
  rewriteEnabled = false,
  className,
}: BeachCanvasShellProps) {
  const initialTileState = useMemo(() => layoutToTileState(initialLayout), [initialLayout]);
  const initialTileSignature = useMemo(
    () => serializeTileStateKey(initialTileState),
    [initialTileState],
  );
  const resolvedManagerUrl = useMemo(() => buildManagerUrl(managerUrl), [managerUrl]);

  return (
    <TileStoreProvider initialState={initialTileState}>
      <ManagerTokenProvider initialToken={managerToken}>
        <BeachCanvasShellInner
          beachId={beachId}
          beachName={beachName}
          backHref={backHref}
          managerUrl={resolvedManagerUrl}
          initialLayout={initialLayout}
          initialSessions={initialSessions}
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
  initialSessions,
  initialTileSignature,
  rewriteEnabled = false,
  className,
}: BeachCanvasShellInnerProps) {
  const { createTile, updateTileMeta, setInteractiveTile } = useTileActions();
  const tileState = useTileState();

  const requestImmediatePersist = useTileLayoutPersistence({
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

      const nodeType = payload.nodeType as TileNodeType;
      createTile({
        id: recordId,
        nodeType,
        position: payload.snappedPosition,
        size: payload.size,
        agentMeta: nodeType === 'agent' ? { role: '', responsibility: '', isEditing: true } : undefined,
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
      requestImmediatePersist();
    },
    [beachId, createTile, requestImmediatePersist, rewriteEnabled],
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
      requestImmediatePersist();
    },
    [beachId, requestImmediatePersist, rewriteEnabled],
  );

  useEffect(() => {
    emitTelemetry('canvas.rewrite.flag-state', {
      privateBeachId: beachId,
      enabled: rewriteEnabled,
    });
  }, [beachId, rewriteEnabled]);

  const interactiveTileId = tileState.interactiveId;
  const interactiveTile = interactiveTileId ? tileState.tiles[interactiveTileId] : null;
  const interactiveSessionTitle = interactiveTile?.sessionMeta?.title ?? null;
  const interactiveSessionId = interactiveTile?.sessionMeta?.sessionId ?? null;
  const interactiveBadgeCode = interactiveSessionId ? interactiveSessionId.slice(0, 5) : null;
  const showInteractiveBadge = Boolean(interactiveBadgeCode);
  const handleClearInteractive = useCallback(() => {
    setInteractiveTile(null);
  }, [setInteractiveTile]);

  useEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }
    const payload = interactiveTile
      ? {
          tileId: interactiveTile.id,
          sessionId: interactiveTile.sessionMeta?.sessionId ?? null,
          title: interactiveTile.sessionMeta?.title ?? null,
        }
      : { tileId: null };
    console.info('[rewrite-2] interactive-state', payload);
  }, [interactiveTile, interactiveTile?.sessionMeta?.sessionId, interactiveTileId]);

  const wrapperClassName = [
    'relative z-0 flex h-full min-h-0 w-full flex-col bg-slate-950 text-slate-200',
    "after:pointer-events-none after:absolute after:inset-0 after:-z-10 after:content-[''] after:bg-[radial-gradient(circle_at_top,rgba(56,189,248,0.18),transparent_60%)]",
    className ?? '',
  ]
    .filter(Boolean)
    .join(' ');

  useEffect(() => {
    if (!initialSessions || initialSessions.length === 0) {
      return;
    }
    for (const session of initialSessions) {
      const tileId = extractTileLinkFromMetadata(session.metadata);
      if (!tileId) {
        continue;
      }
      const tile = tileState.tiles[tileId];
      if (!tile) {
        continue;
      }
      if (tile.sessionMeta?.sessionId === session.session_id) {
        continue;
      }
      const meta = sessionSummaryToTileMeta(session);
      updateTileMeta(tileId, meta);
    }
  }, [initialSessions, tileState.tiles, updateTileMeta]);

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
        <div className="flex items-center gap-3">
          {showInteractiveBadge && interactiveBadgeCode ? (
            <button
              type="button"
              onClick={handleClearInteractive}
              className="inline-flex items-center gap-1.5 rounded-full border border-orange-300/60 bg-orange-400/85 px-2.5 py-0.5 text-[10px] font-semibold text-slate-950 shadow-[0_12px_28px_rgba(249,115,22,0.35)] transition hover:bg-orange-300"
              title="Stop interacting with this session"
            >
              <span className="text-[9px] uppercase tracking-[0.32em]">Interact</span>
              <span className="font-mono text-[11px] tracking-wide">#{interactiveBadgeCode}</span>
              <span
                aria-hidden="true"
                className="ml-0.5 inline-flex h-4 w-4 items-center justify-center rounded-full bg-slate-900/30 text-[11px] font-bold"
              >
                ×
              </span>
              <span className="sr-only">Stop interacting with {interactiveSessionTitle ?? `session ${interactiveBadgeCode}`}</span>
            </button>
          ) : null}
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
