'use client';

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { MouseEvent, PointerEvent } from 'react';
import { Handle, Position } from 'reactflow';
import type { NodeProps } from 'reactflow';
import { ApplicationTile } from '@/components/ApplicationTile';
import { cn } from '@/lib/cn';
import { TILE_HEADER_HEIGHT } from '../constants';
import { useTileActions } from '../store';
import type { TileDescriptor, TileSessionMeta } from '../types';
import { snapSize } from '../utils';
import { emitTelemetry } from '../../../../../private-beach/src/lib/telemetry';

type TileFlowNodeData = {
  tile: TileDescriptor;
  orderIndex: number;
  isActive: boolean;
  isResizing: boolean;
  privateBeachId: string;
  managerUrl: string;
  rewriteEnabled: boolean;
  isInteractive: boolean;
};

type ResizeState = {
  pointerId: number;
  startX: number;
  startY: number;
  width: number;
  height: number;
  lastSize?: { width: number; height: number };
};

function metaEqual(a: TileSessionMeta | null | undefined, b: TileSessionMeta | null | undefined) {
  if (!a && !b) return true;
  if (!a || !b) return false;
  return (
    a.sessionId === b.sessionId &&
    a.title === b.title &&
    a.status === b.status &&
    a.harnessType === b.harnessType &&
    (a.pendingActions ?? null) === (b.pendingActions ?? null)
  );
}

function isResizeHandle(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && target.dataset.tileResizeHandle === 'true';
}

function isInteractiveElement(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  if (target.closest('[data-tile-drag-ignore="true"]')) {
    return true;
  }
  return Boolean(target.closest('button, input, textarea, select, a, label'));
}

type Props = NodeProps<TileFlowNodeData>;

export function TileFlowNode({ data, dragging }: Props) {
  const { tile, orderIndex, isActive, isResizing, isInteractive, privateBeachId, managerUrl, rewriteEnabled } = data;
  const {
    removeTile,
    bringToFront,
    setActiveTile,
    beginResize,
    resizeTile,
    endResize,
    updateTileMeta,
    setInteractiveTile,
    createTile,
  } = useTileActions();
  const resizeStateRef = useRef<ResizeState | null>(null);
  const [hovered, setHovered] = useState(false);
  const isAgent = tile.nodeType === 'agent';
  const agentMeta = useMemo(
    () => tile.agentMeta ?? { role: '', responsibility: '', isEditing: true },
    [tile.agentMeta],
  );
  const [agentRole, setAgentRole] = useState(agentMeta.role);
  const [agentResponsibility, setAgentResponsibility] = useState(agentMeta.responsibility);


  const zIndex = useMemo(() => 10 + orderIndex, [orderIndex]);

  useEffect(() => {
    if (!isAgent) {
      return;
    }
    if (!agentMeta.isEditing) {
      setAgentRole(agentMeta.role);
      setAgentResponsibility(agentMeta.responsibility);
    }
  }, [agentMeta.isEditing, agentMeta.responsibility, agentMeta.role, isAgent]);

  const handleRemove = useCallback(
    (event: MouseEvent<HTMLButtonElement>) => {
      event.stopPropagation();
      emitTelemetry('canvas.tile.remove', {
        privateBeachId,
        tileId: tile.id,
        rewriteEnabled,
      });
      removeTile(tile.id);
    },
    [privateBeachId, removeTile, rewriteEnabled, tile.id],
  );

  const handleAgentSave = useCallback(() => {
    if (!isAgent) return;
    const nextRole = agentRole.trim();
    const nextResp = agentResponsibility.trim();
    if (!nextRole || !nextResp) {
      return;
    }
    createTile({
      id: tile.id,
      agentMeta: { role: nextRole, responsibility: nextResp, isEditing: false },
      focus: false,
    });
  }, [agentResponsibility, agentRole, createTile, isAgent, tile.id]);

  const handleAgentEdit = useCallback(() => {
    if (!isAgent) return;
    createTile({
      id: tile.id,
      agentMeta: { ...agentMeta, isEditing: true },
      focus: false,
    });
  }, [agentMeta, createTile, isAgent, tile.id]);

  const handleAgentCancel = useCallback(() => {
    if (!isAgent) return;
    if (!agentMeta.role && !agentMeta.responsibility) {
      removeTile(tile.id);
      return;
    }
    setAgentRole(agentMeta.role);
    setAgentResponsibility(agentMeta.responsibility);
    createTile({
      id: tile.id,
      agentMeta: { ...agentMeta, isEditing: false },
      focus: false,
    });
  }, [agentMeta, createTile, isAgent, removeTile, tile.id]);

  const handlePointerDown = useCallback(
    (event: PointerEvent<HTMLElement>) => {
      if (event.button !== 0) {
        return;
      }
      const target = event.target;
      if (isInteractive && isInteractiveElement(target) && !isResizeHandle(target)) {
        // Keep events inside interactive UI (inputs, buttons) from initiating drags.
        event.stopPropagation();
        return;
      }
      bringToFront(tile.id);
      setActiveTile(tile.id);
      if (isInteractiveElement(event.target)) {
        event.stopPropagation();
      }
    },
    [bringToFront, isInteractive, setActiveTile, tile.id],
  );

  const handleResizePointerDown = useCallback(
    (event: PointerEvent<HTMLButtonElement>) => {
      event.preventDefault();
      const target = event.target;
      const allowWhileInteractive = isResizeHandle(target);
      if (isInteractive && !allowWhileInteractive) {
        return;
      }
      event.stopPropagation();
      bringToFront(tile.id);
      setActiveTile(tile.id);
      beginResize(tile.id);
      const { width, height } = tile.size;
      resizeStateRef.current = {
        pointerId: event.pointerId,
        startX: event.clientX,
        startY: event.clientY,
        width,
        height,
        lastSize: { width, height },
      };
      try {
        event.currentTarget.setPointerCapture(event.pointerId);
      } catch {
        // ignore pointer capture issues
      }
    },
    [beginResize, bringToFront, isInteractive, setActiveTile, tile.id, tile.size],
  );

  const handleResizePointerMove = useCallback(
    (event: PointerEvent<HTMLButtonElement>) => {
      const state = resizeStateRef.current;
      if (!state || state.pointerId !== event.pointerId) {
        return;
      }
      const deltaX = event.clientX - state.startX;
      const deltaY = event.clientY - state.startY;
      const nextSize = snapSize({
        width: state.width + deltaX,
        height: state.height + deltaY,
      });
      state.lastSize = nextSize;
      resizeTile(tile.id, nextSize);
    },
    [resizeTile, tile.id],
  );

  const releaseResizePointer = useCallback((event: PointerEvent<HTMLButtonElement>) => {
    try {
      if (event.currentTarget.hasPointerCapture(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId);
      }
    } catch {
      // ignore release errors
    }
  }, []);

  const handleResizePointerUp = useCallback(
    (event: PointerEvent<HTMLButtonElement>) => {
      const state = resizeStateRef.current;
      if (!state || state.pointerId !== event.pointerId) {
        return;
      }
      releaseResizePointer(event);
      endResize(tile.id);
      if (state.lastSize) {
        emitTelemetry('canvas.resize.stop', {
          privateBeachId,
          tileId: tile.id,
          width: state.lastSize.width,
          height: state.lastSize.height,
          rewriteEnabled,
        });
        console.info('[ws-d] tile resized', {
          privateBeachId,
          tileId: tile.id,
          size: { ...state.lastSize },
          rewriteEnabled,
        });
      }
      resizeStateRef.current = null;
    },
    [endResize, privateBeachId, releaseResizePointer, rewriteEnabled, tile.id],
  );

  const handleResizePointerCancel = useCallback(
    (event: PointerEvent<HTMLButtonElement>) => {
      releaseResizePointer(event);
      endResize(tile.id);
      resizeStateRef.current = null;
    },
    [endResize, releaseResizePointer, tile.id],
  );

  const handlePointerEnterNode = useCallback(() => {
    setHovered(true);
  }, []);

  const handlePointerLeaveNode = useCallback(() => {
    setHovered(false);
    if (isInteractive) {
      setInteractiveTile(null);
    }
  }, [isInteractive, setInteractiveTile]);

  useEffect(() => {
    if (!hovered) {
      return;
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      const isSpace = event.key === ' ' || event.key === 'Spacebar' || event.code === 'Space';
      if (isSpace) {
        event.preventDefault();
        setInteractiveTile(isInteractive ? null : tile.id);
        return;
      }
      if (event.key === 'Escape') {
        if (isInteractive) {
          event.preventDefault();
          setInteractiveTile(null);
        }
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, [hovered, isInteractive, setInteractiveTile, tile.id]);

  const title = tile.sessionMeta?.title ?? tile.sessionMeta?.sessionId ?? 'Application Tile';
  const subtitle = useMemo(() => {
    if (!tile.sessionMeta) return 'Disconnected';
    if (tile.sessionMeta.status) return tile.sessionMeta.status;
    if (tile.sessionMeta.harnessType) return tile.sessionMeta.harnessType;
    return 'Attached';
  }, [tile.sessionMeta]);
  const headerTitle = isAgent ? 'Agent' : title;
  const headerSubtitle = isAgent ? agentMeta.role || 'Define this agent' : subtitle;
  const showAgentEditor = isAgent && (agentMeta.isEditing || (!agentMeta.role && !agentMeta.responsibility));
  const shouldRenderViewer = !isAgent || !showAgentEditor;

  const handleMetaChange = useCallback(
    (meta: TileSessionMeta | null) => {
      const current = tile.sessionMeta ?? null;
      if (metaEqual(current, meta)) {
        return;
      }
      updateTileMeta(tile.id, meta);
    },
    [tile.id, tile.sessionMeta, updateTileMeta],
  );

  const handleToggleInteractive = useCallback(() => {
    if (isInteractive) {
      setInteractiveTile(null);
      return;
    }
    bringToFront(tile.id);
    setActiveTile(tile.id);
    setInteractiveTile(tile.id);
  }, [bringToFront, isInteractive, setActiveTile, setInteractiveTile, tile.id]);

  const nodeClass = cn(
    'group relative flex h-full w-full select-none flex-col overflow-hidden rounded-2xl border border-slate-700/60 bg-slate-950/80 text-slate-200 shadow-[0_28px_80px_rgba(2,6,23,0.6)] backdrop-blur-xl transition-all duration-200',
    isActive && 'border-sky-400/60 shadow-[0_32px_90px_rgba(14,165,233,0.35)]',
    isResizing && 'cursor-[se-resize]',
    isInteractive && 'border-sky-300/70 shadow-[0_32px_90px_rgba(56,189,248,0.45)] cursor-auto',
  );

  return (
    <article
      className={nodeClass}
      style={{ width: '100%', height: '100%', zIndex }}
      data-testid={`rf__node-tile:${tile.id}`}
      data-tile-id={tile.id}
      onPointerDown={handlePointerDown}
      onPointerEnter={handlePointerEnterNode}
      onPointerLeave={handlePointerLeaveNode}
      data-tile-interactive={isInteractive ? 'true' : 'false'}
    >
      <header
        className="flex min-h-[44px] items-center justify-between border-b border-white/10 bg-slate-900/80 px-4 py-2.5 backdrop-blur"
        style={{ minHeight: TILE_HEADER_HEIGHT }}
      >
        <div className="flex min-w-0 flex-col gap-1">
          <span className="truncate text-sm font-semibold text-white/90" title={headerTitle}>
            {headerTitle}
          </span>
          {headerSubtitle ? (
            <small className="truncate text-[11px] uppercase tracking-[0.18em] text-slate-400">
              {headerSubtitle}
            </small>
          ) : null}
          {isInteractive ? (
            <span className="mt-0.5 inline-flex items-center gap-2 text-[10px] font-semibold uppercase tracking-[0.28em] text-sky-200">
              Live Control
            </span>
          ) : null}
        </div>
        <div className="flex items-center gap-2">
          {isAgent && !showAgentEditor ? (
            <button
              type="button"
              onClick={handleAgentEdit}
              data-tile-drag-ignore="true"
              className="inline-flex h-7 items-center justify-center rounded-full border border-indigo-400/40 bg-indigo-500/10 px-3 text-[10px] font-semibold uppercase tracking-[0.24em] text-indigo-100"
            >
              Edit
            </button>
          ) : null}
          <button
            type="button"
            onClick={handleToggleInteractive}
            aria-pressed={isInteractive}
            data-tile-drag-ignore="true"
            className={cn(
              'inline-flex h-7 items-center justify-center rounded-full border px-3 text-[10px] font-semibold uppercase tracking-[0.24em] transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60',
              isInteractive
                ? 'border-sky-400/70 bg-sky-500/20 text-sky-50'
                : 'border-white/15 bg-white/5 text-slate-300 hover:border-white/30 hover:text-white',
            )}
            title="Toggle interactive mode (Space)"
          >
            {isInteractive ? 'Done' : 'Interact'}
          </button>
          <button
            type="button"
            className="inline-flex h-7 w-7 items-center justify-center rounded-full border border-red-500/40 bg-red-500/15 text-base font-semibold text-red-200 transition hover:border-red-400/70 hover:bg-red-500/25 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-400/60"
            onClick={handleRemove}
            data-tile-drag-ignore="true"
            title="Remove tile"
          >
            ×
          </button>
        </div>
      </header>
      {isAgent ? (
        <div
          className="border-b border-white/5 bg-slate-950/70 px-4 py-3 text-sm text-slate-300"
          data-tile-drag-ignore="true"
        >
          {showAgentEditor ? (
            <div className="space-y-2">
              <label className="text-xs font-semibold uppercase tracking-[0.2em] text-slate-400" htmlFor={`agent-role-${tile.id}`}>
                Role
              </label>
              <input
                id={`agent-role-${tile.id}`}
                value={agentRole}
                onChange={(event) => setAgentRole(event.target.value)}
                className="w-full rounded border border-white/10 bg-white/5 px-3 py-2 text-sm text-white focus:border-indigo-400 focus:outline-none"
                placeholder="e.g. Deploy orchestrator"
              />
              <label className="text-xs font-semibold uppercase tracking-[0.2em] text-slate-400" htmlFor={`agent-resp-${tile.id}`}>
                Responsibility
              </label>
              <textarea
                id={`agent-resp-${tile.id}`}
                value={agentResponsibility}
                onChange={(event) => setAgentResponsibility(event.target.value)}
                rows={3}
                className="w-full rounded border border-white/10 bg-white/5 px-3 py-2 text-sm text-white focus:border-indigo-400 focus:outline-none"
                placeholder="Describe how this agent should manage connected sessions"
              />
              <div className="flex gap-2 pt-1 text-xs">
                <button
                  type="button"
                  onClick={handleAgentSave}
                  className="flex-1 rounded bg-indigo-600 px-3 py-2 font-semibold text-white disabled:opacity-40"
                  disabled={!agentRole.trim() || !agentResponsibility.trim()}
                >
                  Save
                </button>
                <button
                  type="button"
                  onClick={handleAgentCancel}
                  className="flex-1 rounded border border-white/15 px-3 py-2 font-semibold text-slate-200"
                >
                  Cancel
                </button>
              </div>
            </div>
          ) : (
            <div className="flex flex-col gap-2 text-xs">
              <div>
                <p className="font-semibold uppercase tracking-[0.2em] text-slate-400">Role</p>
                <p className="text-sm text-white/90">{agentMeta.role}</p>
              </div>
              <div>
                <p className="font-semibold uppercase tracking-[0.2em] text-slate-400">Responsibility</p>
                <p className="text-sm text-white/90">{agentMeta.responsibility}</p>
              </div>
              <p className="text-[11px] text-slate-400">
                Drag the right connector to an application or another agent to define how this agent should manage it.
              </p>
            </div>
          )}
        </div>
      ) : null}
      {shouldRenderViewer ? (
        <section
          className={cn(
            'flex flex-1 flex-col gap-3 overflow-hidden bg-slate-950/60 transition-opacity',
            isInteractive ? 'pointer-events-auto' : 'pointer-events-none opacity-[0.98] select-none',
          )}
          data-tile-drag-ignore="true"
        >
          <ApplicationTile
            tileId={tile.id}
            privateBeachId={privateBeachId}
            managerUrl={managerUrl}
            sessionMeta={tile.sessionMeta ?? null}
            onSessionMetaChange={handleMetaChange}
          />
        </section>
      ) : (
        <div className="flex flex-1 items-center justify-center bg-slate-950/60 px-4 text-center text-sm text-slate-400" data-tile-drag-ignore="true">
          Save this agent&rsquo;s role and responsibility to start connecting it to sessions.
        </div>
      )}
      <button
        type="button"
        className="absolute bottom-3 right-3 z-10 h-5 w-5 cursor-nwse-resize rounded-md border border-sky-400/40 bg-[radial-gradient(circle_at_top_left,rgba(56,189,248,0.6),rgba(56,189,248,0.05))] text-transparent transition hover:border-sky-400/60 hover:shadow-[0_0_12px_rgba(56,189,248,0.45)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60"
        aria-label="Resize tile"
        onPointerDown={handleResizePointerDown}
        onPointerMove={handleResizePointerMove}
        onPointerUp={handleResizePointerUp}
        onPointerCancel={handleResizePointerCancel}
        data-tile-drag-ignore="true"
        data-tile-resize-handle="true"
      />
      <Handle
        type="target"
        position={Position.Left}
        className="h-3 w-3 border-none bg-indigo-200/80 transition hover:bg-indigo-300"
        onPointerDown={(event) => event.stopPropagation()}
      />
      {isAgent ? (
        <Handle
          type="source"
          position={Position.Right}
          className="h-3 w-3 border-none bg-indigo-500 transition hover:bg-indigo-400"
          onPointerDown={(event) => event.stopPropagation()}
        />
      ) : null}
    </article>
  );
}
