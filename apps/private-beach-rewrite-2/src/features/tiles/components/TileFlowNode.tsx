'use client';

import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, MouseEvent, PointerEvent } from 'react';
import { Handle, Position, useStore } from 'reactflow';
import type { NodeProps, ReactFlowState } from 'reactflow';
import { ApplicationTile } from '@/components/ApplicationTile';
import { cn } from '@/lib/cn';
import { listSessions, onboardAgent, updateSessionMetadata, updateSessionRoleById } from '@/lib/api';
import { buildManagerUrl, useManagerToken } from '@/hooks/useManagerToken';
import { TILE_HEADER_HEIGHT } from '../constants';
import { useTileActions } from '../store';
import type { AgentMetadata, TileDescriptor, TileSessionMeta, TileViewportSnapshot } from '../types';
import { snapSize } from '../utils';
import { emitTelemetry } from '../../../../../private-beach/src/lib/telemetry';
import { computeAutoResizeSize } from '../autoResize';
import { buildSessionMetadataWithTile } from '../sessionMeta';

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

const HANDLE_BASE_CLASS =
  'pointer-events-auto h-7 w-7 rounded-full border-[1.5px] transition shadow-[0_0_12px_rgba(15,23,42,0.5)] flex items-center justify-center text-[10px] font-semibold';
const TARGET_HANDLE_CLASS = `${HANDLE_BASE_CLASS} border-white/70 bg-slate-900/90 text-white hover:border-white hover:bg-slate-900`;
const SOURCE_HANDLE_CLASS = `${HANDLE_BASE_CLASS} border-indigo-300/80 bg-indigo-500/90 text-white hover:border-indigo-100 hover:bg-indigo-400`;

type HandleDef = { key: 'top' | 'right' | 'bottom' | 'left'; pos: Position; style: CSSProperties };

const HANDLE_DEFS: HandleDef[] = [
  { key: 'top', pos: Position.Top, style: { top: -12, left: '50%', transform: 'translate(-50%, -50%)' } },
  { key: 'right', pos: Position.Right, style: { right: -12, top: '50%', transform: 'translate(50%, -50%)' } },
  { key: 'bottom', pos: Position.Bottom, style: { bottom: -12, left: '50%', transform: 'translate(-50%, 50%)' } },
  { key: 'left', pos: Position.Left, style: { left: -12, top: '50%', transform: 'translate(-50%, -50%)' } },
];

const AUTO_RESIZE_TOLERANCE_PX = 1;

function generateTraceId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `trace-${Math.random().toString(36).slice(2, 10)}`;
}

type LegacyAgentMeta = AgentMetadata & {
  traceEnabled?: boolean;
  traceId?: string | null;
};

function coerceAgentTrace(meta?: LegacyAgentMeta | null): AgentMetadata['trace'] | undefined {
  if (meta?.trace && typeof meta.trace.enabled === 'boolean') {
    if (!meta.trace.enabled) {
      return { enabled: false };
    }
    return {
      enabled: true,
      trace_id: meta.trace.trace_id ?? undefined,
    };
  }
  if (typeof meta?.traceEnabled === 'boolean') {
    if (!meta.traceEnabled) {
      return { enabled: false };
    }
    return {
      enabled: true,
      trace_id: meta.traceId ?? undefined,
    };
  }
  return undefined;
}

function normalizeAgentMeta(meta?: LegacyAgentMeta | null): AgentMetadata {
  const trace = coerceAgentTrace(meta);
  const normalized: AgentMetadata = {
    role: meta?.role ?? '',
    responsibility: meta?.responsibility ?? '',
    isEditing: meta?.isEditing ?? true,
  };
  if (trace) {
    normalized.trace = trace;
  }
  return normalized;
}

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
  if (target.closest('[data-terminal-root="true"], [data-terminal-content="true"]')) {
    return true;
  }
  return Boolean(target.closest('button, input, textarea, select, a, label'));
}

function logAutoResizeEvent(tileId: string, step: string, detail: Record<string, unknown> = {}) {
  if (typeof window === 'undefined') {
    return;
  }
  try {
    console.info('[tile][auto-resize]', step, JSON.stringify({ tileId, ...detail }));
  } catch (error) {
    console.info('[tile][auto-resize]', step, { tileId, ...detail });
  }
}

type Props = NodeProps<TileFlowNodeData>;

const zoomSelector = (state: ReactFlowState) => state.transform[2] ?? 1;

function TileFlowNodeImpl({ data, dragging }: Props) {
  const {
    tile,
    orderIndex,
    isActive,
    isResizing,
    isInteractive,
    privateBeachId,
    managerUrl,
    rewriteEnabled,
  } = data;
  const managerBaseUrl = useMemo(() => buildManagerUrl(managerUrl), [managerUrl]);
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
    updateTileViewport,
  } = useTileActions();
  const nodeRef = useRef<HTMLElement | null>(null);
  const viewportMetricsRef = useRef<TileViewportSnapshot | null>(null);
  const resizeStateRef = useRef<ResizeState | null>(null);
  const lastResizeClickRef = useRef<number>(0);
  const [hovered, setHovered] = useState(false);
  const [terminalHover, setTerminalHover] = useState(false);
  const isAgent = tile.nodeType === 'agent';
  const agentMeta = useMemo(() => normalizeAgentMeta(tile.agentMeta ?? null), [tile.agentMeta]);
  const [agentRole, setAgentRole] = useState(agentMeta.role);
  const [agentResponsibility, setAgentResponsibility] = useState(agentMeta.responsibility);
  const { token: managerToken, refresh: refreshManagerToken } = useManagerToken();
  const [agentTraceEnabled, setAgentTraceEnabled] = useState(Boolean(agentMeta.trace?.enabled));
  const [agentTraceId, setAgentTraceId] = useState<string | null>(agentMeta.trace?.trace_id ?? null);
  const [agentSaveState, setAgentSaveState] = useState<'idle' | 'saving'>('idle');
  const [agentSaveNotice, setAgentSaveNotice] = useState<string | null>(null);
  const zoom = useStore(zoomSelector);


  const zIndex = useMemo(() => 10 + orderIndex, [orderIndex]);

  useEffect(() => {
    if (!isAgent) {
      return;
    }
    if (!agentMeta.isEditing) {
      setAgentRole(agentMeta.role);
      setAgentResponsibility(agentMeta.responsibility);
      setAgentTraceEnabled(Boolean(agentMeta.trace?.enabled));
      setAgentTraceId(agentMeta.trace?.trace_id ?? null);
    }
  }, [
    agentMeta.isEditing,
    agentMeta.responsibility,
    agentMeta.role,
    agentMeta.trace?.enabled,
    agentMeta.trace?.trace_id,
    isAgent,
  ]);

  useEffect(() => {
    if (agentMeta.isEditing) {
      setAgentSaveNotice(null);
    }
  }, [agentMeta.isEditing]);

  const handleTraceToggle = useCallback(
    (enabled: boolean) => {
      setAgentTraceEnabled(enabled);
      if (enabled) {
        if (!agentTraceId) {
          setAgentTraceId(generateTraceId());
        }
        return;
      }
      setAgentTraceId(null);
    },
    [agentTraceId],
  );

  const handleTraceRefresh = useCallback(() => {
    setAgentTraceId(generateTraceId());
  }, []);

  const ensureTraceState = useCallback(() => {
    if (!agentTraceEnabled) {
      if (agentTraceId !== null) {
        setAgentTraceId(null);
      }
      return { enabled: false, traceId: null as string | null };
    }
    if (agentTraceId) {
      return { enabled: true, traceId: agentTraceId };
    }
    const id = generateTraceId();
    setAgentTraceId(id);
    return { enabled: true, traceId: id };
  }, [agentTraceEnabled, agentTraceId]);

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

  const runAgentOnboarding = useCallback(
    async (metaPayload: AgentMetadata) => {
      if (!tile.sessionMeta?.sessionId || !privateBeachId) {
        return;
      }
      setAgentSaveState('saving');
      setAgentSaveNotice(null);
      try {
        const token = managerToken ?? (await refreshManagerToken());
        if (!token) {
          console.warn('[agent-tile] unable to acquire manager token for onboarding', {
            tileId: tile.id,
          });
          return;
        }
        const sessionId = tile.sessionMeta.sessionId;
        const sessions = await listSessions(privateBeachId, token, managerBaseUrl);
        const summary = sessions.find((session) => session.session_id === sessionId) ?? null;
        if (!summary) {
          console.warn('[agent-tile] agent session not found on manager', {
            tileId: tile.id,
            sessionId,
          });
          return;
        }
        await updateSessionRoleById(
          sessionId,
          'agent',
          token,
          managerBaseUrl,
          summary.metadata,
          summary.location_hint ?? null,
        );
        const traceId = metaPayload.trace?.enabled ? metaPayload.trace.trace_id ?? undefined : undefined;
        const onboarding = await onboardAgent(
          sessionId,
          'pong',
          ['agent'],
          token,
          managerBaseUrl,
          undefined,
          traceId,
        );
        const metadataPayload = buildSessionMetadataWithTile(summary.metadata, tile.id, tile.sessionMeta ?? {});
        metadataPayload.role = 'agent';
        metadataPayload.agent = {
          profile: 'pong',
          prompt_pack: onboarding.prompt_pack ?? null,
          mcp_bridges: onboarding.mcp_bridges ?? [],
        };
        if (metaPayload.trace?.enabled) {
          metadataPayload.agent.trace = {
            enabled: true,
            trace_id: metaPayload.trace.trace_id ?? null,
          };
        }
        await updateSessionMetadata(
          sessionId,
          {
            metadata: metadataPayload,
            location_hint: summary.location_hint ?? null,
          },
          token,
          managerBaseUrl,
        );
        setAgentSaveNotice('Agent onboarding updated.');
      } catch (error) {
        console.warn('[agent-tile] onboarding failed', {
          tileId: tile.id,
          sessionId: tile.sessionMeta?.sessionId,
          error,
        });
      } finally {
        setAgentSaveState('idle');
      }
    },
    [managerBaseUrl, managerToken, privateBeachId, refreshManagerToken, tile.id, tile.sessionMeta],
  );

  const handleAgentSave = useCallback(() => {
    if (!isAgent) return;
    const nextRole = agentRole.trim();
    const nextResp = agentResponsibility.trim();
    if (!nextRole || !nextResp) {
      return;
    }
    const traceState = ensureTraceState();
    const nextMeta: AgentMetadata = {
      role: nextRole,
      responsibility: nextResp,
      isEditing: false,
    };
    if (traceState.enabled) {
      nextMeta.trace = {
        enabled: true,
        trace_id: traceState.traceId ?? undefined,
      };
    }
    createTile({
      id: tile.id,
      agentMeta: nextMeta,
      focus: false,
    });
    if (tile.sessionMeta?.sessionId && privateBeachId) {
      void runAgentOnboarding(nextMeta);
    }
  }, [
    agentResponsibility,
    agentRole,
    createTile,
    ensureTraceState,
    isAgent,
    privateBeachId,
    runAgentOnboarding,
    tile.id,
    tile.sessionMeta,
  ]);

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
    setAgentTraceEnabled(Boolean(agentMeta.trace?.enabled));
    setAgentTraceId(agentMeta.trace?.trace_id ?? null);
    createTile({
      id: tile.id,
      agentMeta: { ...agentMeta, isEditing: false },
      focus: false,
    });
  }, [agentMeta, createTile, isAgent, removeTile, tile.id]);

  const handleAutoResize = useCallback((force = false) => {
    if (isResizing && !force) {
      logAutoResizeEvent(tile.id, 'skip-resizing');
      return;
    }
    const viewportMetrics = viewportMetricsRef.current;
    logAutoResizeEvent(tile.id, 'attempt', {
      hasMetrics: Boolean(viewportMetrics),
      zoom: zoom ?? null,
    });
    if (!viewportMetrics) {
      logAutoResizeEvent(tile.id, 'missing-metrics');
      return;
    }
    const hostRows = viewportMetrics.hostRows ?? null;
    const hostCols = viewportMetrics.hostCols ?? null;
    const pixelsPerRow = viewportMetrics.pixelsPerRow ?? null;
    const pixelsPerCol = viewportMetrics.pixelsPerCol ?? null;
    const hostWidthPx = viewportMetrics.hostWidthPx ?? null;
    const hostHeightPx = viewportMetrics.hostHeightPx ?? null;
    const container = nodeRef.current;
    if (!container) {
      logAutoResizeEvent(tile.id, 'missing-container');
      return;
    }
    const terminalRoot = container.querySelector<HTMLElement>(
      `[data-terminal-root="true"][data-terminal-tile="${tile.id}"]`,
    );
    const terminalContent = terminalRoot?.querySelector<HTMLElement>('[data-terminal-content="true"]') ?? terminalRoot;
    const terminal = terminalContent ?? terminalRoot;
    const beachTerminal = terminalRoot?.querySelector<HTMLElement>('.beach-terminal') ?? terminal;
    if (!terminal) {
      logAutoResizeEvent(tile.id, 'missing-terminal');
      return;
    }
    const tileRect = container.getBoundingClientRect();
    const terminalRect = terminal.getBoundingClientRect();
    if (tileRect.width <= 0 || tileRect.height <= 0 || terminalRect.width <= 0 || terminalRect.height <= 0) {
      logAutoResizeEvent(tile.id, 'invalid-rect', { tileRect, terminalRect });
      return;
    }
    const zoomFactor = zoom && zoom > 0 ? zoom : 1;
    const tileWidthPx = tileRect.width / zoomFactor;
    const tileHeightPx = tileRect.height / zoomFactor;
    const terminalWidthPx = terminalRect.width / zoomFactor;
    const terminalHeightPx = terminalRect.height / zoomFactor;
    let paddingX = 0;
    let paddingY = 0;
    if (beachTerminal) {
      const computed = window.getComputedStyle(beachTerminal);
      const px = Number.parseFloat(computed.paddingLeft ?? '0');
      const pr = Number.parseFloat(computed.paddingRight ?? '0');
      const pt = Number.parseFloat(computed.paddingTop ?? '0');
      const pb = Number.parseFloat(computed.paddingBottom ?? '0');
      paddingX = (Number.isFinite(px) ? px : 0) + (Number.isFinite(pr) ? pr : 0);
      paddingY = (Number.isFinite(pt) ? pt : 0) + (Number.isFinite(pb) ? pb : 0);
    }
    const chromeWidthPx = Math.max(0, tileWidthPx - terminalWidthPx) + paddingX;
    const chromeHeightPx = Math.max(0, tileHeightPx - terminalHeightPx) + paddingY;
    const nextSize = computeAutoResizeSize({
      metrics: viewportMetrics,
      chromeWidthPx,
      chromeHeightPx,
    });
    if (!nextSize) {
      logAutoResizeEvent(tile.id, 'compute-failed', {
        chromeWidthPx,
        chromeHeightPx,
        hostRows,
        hostCols,
        hostWidthPx,
        hostHeightPx,
        pixelsPerRow,
        pixelsPerCol,
      });
      return;
    }
    if (nextSize.width === tile.size.width && nextSize.height === tile.size.height) {
      logAutoResizeEvent(tile.id, 'no-op', nextSize);
      return;
    }
    const deltaWidth = Math.abs(nextSize.width - tile.size.width);
    const deltaHeight = Math.abs(nextSize.height - tile.size.height);
    if (deltaWidth <= AUTO_RESIZE_TOLERANCE_PX && deltaHeight <= AUTO_RESIZE_TOLERANCE_PX) {
      logAutoResizeEvent(tile.id, 'tolerance-skip', {
        size: nextSize,
        current: tile.size,
      });
      return;
    }
    logAutoResizeEvent(tile.id, 'apply', {
      size: nextSize,
      chromeWidthPx,
      chromeHeightPx,
      zoom: zoom ?? 1,
    });
    resizeTile(tile.id, nextSize);
    if (beachTerminal) {
      beachTerminal.scrollTo?.({ top: 0, left: 0, behavior: 'auto' });
      beachTerminal.scrollTop = 0;
      beachTerminal.scrollLeft = 0;
      if (terminal && terminal !== beachTerminal) {
        terminal.scrollTo?.({ top: 0, left: 0, behavior: 'auto' });
        terminal.scrollTop = 0;
        terminal.scrollLeft = 0;
      }
      logAutoResizeEvent(tile.id, 'reset-scroll', {
        scrollTop: beachTerminal.scrollTop,
        scrollLeft: beachTerminal.scrollLeft,
      });
    }
    emitTelemetry('canvas.resize.auto', {
      privateBeachId,
      tileId: tile.id,
      hostRows,
      hostCols,
      viewportRows: viewportMetrics.viewportRows ?? null,
      viewportCols: viewportMetrics.viewportCols ?? null,
      pixelsPerRow,
      pixelsPerCol,
      hostWidthPx,
      hostHeightPx,
      zoom: zoom ?? 1,
      rewriteEnabled,
      size: nextSize,
    });
  }, [isResizing, nodeRef, privateBeachId, resizeTile, rewriteEnabled, tile.id, tile.size.height, tile.size.width, zoom]);

  const handleResizeDoubleClick = useCallback(
    (event: MouseEvent<HTMLButtonElement>) => {
      event.preventDefault();
      event.stopPropagation();
      logAutoResizeEvent(tile.id, 'double-click');
      handleAutoResize(true);
    },
    [handleAutoResize, tile.id],
  );
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
      const now = typeof performance !== 'undefined' ? performance.now() : Date.now();
      const sinceLastClick = now - lastResizeClickRef.current;
      lastResizeClickRef.current = now;
      if (sinceLastClick > 0 && sinceLastClick < 320) {
        event.preventDefault();
        event.stopPropagation();
        logAutoResizeEvent(tile.id, 'pointer-double-click', { deltaMs: Number(sinceLastClick.toFixed(2)) });
        handleAutoResize(true);
        return;
      }
      if (event.detail >= 2) {
        event.preventDefault();
        event.stopPropagation();
        logAutoResizeEvent(tile.id, 'pointer-detail', { detail: event.detail });
        handleAutoResize(true);
        return;
      }
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
    [beginResize, bringToFront, handleAutoResize, isInteractive, setActiveTile, tile.id, tile.size],
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

  useEffect(() => {
    if (!hovered) {
      return;
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      const isSpace = event.key === ' ' || event.key === 'Spacebar' || event.code === 'Space';
      if (!isSpace) {
        return;
      }
      // Space should enter interactive mode when hovered, but not exit it.
      // If already interactive, allow the event to propagate so inputs receive spaces.
      if (isInteractive) {
        return;
      }
      event.preventDefault();
      setInteractiveTile(tile.id);
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, [hovered, isInteractive, setInteractiveTile, tile.id]);

  useEffect(() => {
    if (!isInteractive) {
      return;
    }
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') {
        return;
      }
      event.preventDefault();
      setInteractiveTile(null);
    };
    window.addEventListener('keydown', handleEscape);
    return () => {
      window.removeEventListener('keydown', handleEscape);
    };
  }, [isInteractive, setInteractiveTile]);

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

  const handleViewportMetricsChange = useCallback(
    (snapshot: TileViewportSnapshot | null) => {
      viewportMetricsRef.current = snapshot;
      updateTileViewport(tile.id, snapshot);
    },
    [tile.id, updateTileViewport],
  );

  const showInteractOverlay = !isAgent && !isInteractive && terminalHover && !dragging;

  const nodeClass = cn(
    // Avoid transitioning transform while dragging (can cause flicker).
    // Limit transitions to visual properties only.
    'group relative flex h-full w-full flex-col overflow-visible rounded-2xl border border-slate-700/60 bg-slate-950/80 text-slate-200 shadow-[0_28px_80px_rgba(2,6,23,0.6)] backdrop-blur-xl transition-[box-shadow,border-color] duration-150',
    !isInteractive && 'select-none',
    // Remove blue glow on active tiles
    // Previously: border-sky-400/60 shadow-[0_32px_90px_rgba(14,165,233,0.35)]
    isResizing && 'cursor-[se-resize]',
    isInteractive && 'border-amber-400/80 shadow-[0_32px_90px_rgba(251,146,60,0.45)] ring-1 ring-amber-300/70 cursor-auto',
    dragging && 'transition-none shadow-none backdrop-blur-0',
  );

  return (
    <article
      ref={nodeRef}
      className={nodeClass}
      style={{
        width: '100%',
        height: '100%',
        zIndex,
        willChange: 'transform',
        transform: dragging ? 'translateZ(0)' : undefined,
        backfaceVisibility: 'hidden',
        transformStyle: 'preserve-3d',
        contain: dragging ? ('layout paint' as any) : undefined,
        // Reduce compositor work and avoid filter repainting while dragging
        filter: dragging ? 'none' : undefined,
        backdropFilter: dragging ? 'none' : undefined,
        isolation: 'isolate',
      }}
      data-testid={`rf__node-tile:${tile.id}`}
      data-tile-id={tile.id}
      onPointerDown={handlePointerDown}
      onPointerDownCapture={(event) => {
        // Keep React Flow from starting node-drag in capture phase when interacting
        // with inner, interactive elements (e.g., terminal text selection).
        const target = event.target as Element | null;
        if (isInteractive && isInteractiveElement(target) && !isResizeHandle(target)) {
          event.stopPropagation();
        }
      }}
      onPointerEnter={() => setHovered(true)}
      onPointerLeave={() => setHovered(false)}
      data-tile-interactive={isInteractive ? 'true' : 'false'}
    >
      <div
        className={cn(
          'pointer-events-none absolute inset-0 z-30 flex items-center justify-center rounded-2xl bg-slate-950/40 transition-opacity duration-150',
          showInteractOverlay ? 'opacity-100' : 'opacity-0'
        )}
        aria-hidden={!showInteractOverlay}
      >
        <button
          type="button"
          onClick={handleToggleInteractive}
          className="pointer-events-auto inline-flex items-center gap-2 rounded-full border border-amber-200/70 bg-amber-300/90 px-4 py-2 text-xs font-semibold uppercase tracking-[0.28em] text-slate-900 shadow-lg transition hover:bg-amber-200"
          data-tile-drag-ignore="true"
        >
          Interact
        </button>
      </div>
      <header
        className="rf-drag-handle flex min-h-[44px] items-center justify-between border-b border-white/10 bg-slate-900/80 px-4 py-2.5 backdrop-blur"
        style={{ minHeight: TILE_HEADER_HEIGHT, backdropFilter: dragging ? 'none' as any : undefined }}
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
            <span className="mt-0.5 inline-flex items-center gap-2 text-[10px] font-semibold uppercase tracking-[0.28em] text-amber-200">
              Live Control
            </span>
          ) : null}
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="inline-flex h-7 w-7 items-center justify-center rounded-full border border-sky-400/40 bg-sky-500/10 text-[11px] font-semibold uppercase tracking-[0.2em] text-sky-100 transition hover:border-sky-300/70 hover:bg-sky-500/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60"
            title="Resize tile to host viewport"
            aria-label="Auto resize tile"
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              handleAutoResize(true);
            }}
            onDoubleClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              handleAutoResize(true);
            }}
            data-tile-drag-ignore="true"
          >
            ⇱
          </button>
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
          {agentSaveState === 'saving' ? (
            <p className="mb-2 rounded border border-sky-400/40 bg-sky-500/10 px-3 py-1 text-[11px] uppercase tracking-[0.2em] text-sky-100">
              Updating agent…
            </p>
          ) : null}
          {agentSaveNotice ? (
            <p className="mb-2 rounded border border-emerald-400/30 bg-emerald-500/10 px-3 py-1 text-[11px] text-emerald-100">
              {agentSaveNotice}
            </p>
          ) : null}
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
              <label className="mt-2 flex items-center gap-2 text-[11px] font-semibold uppercase tracking-[0.2em] text-slate-400">
                <input
                  type="checkbox"
                  checked={agentTraceEnabled}
                  onChange={(event) => handleTraceToggle(event.target.checked)}
                  className="h-4 w-4 rounded border border-white/20 bg-slate-900 text-indigo-400 focus:ring-indigo-400"
                />
                <span>Trace Logging</span>
              </label>
              {agentTraceEnabled ? (
                <div className="flex items-center justify-between rounded-md border border-white/10 bg-white/5 px-3 py-2 text-[11px] text-slate-300">
                  <span className="font-mono text-[10px]">
                    {(agentTraceId ?? 'pending…').slice(0, 36)}
                  </span>
                  <button
                    type="button"
                    className="text-[10px] font-semibold uppercase tracking-[0.2em] text-sky-300"
                    onClick={handleTraceRefresh}
                  >
                    Refresh
                  </button>
                </div>
              ) : null}
              <div className="flex gap-2 pt-1 text-xs">
                <button
                  type="button"
                  onClick={handleAgentSave}
                  className="flex-1 rounded bg-indigo-600 px-3 py-2 font-semibold text-white disabled:opacity-40"
                  disabled={!agentRole.trim() || !agentResponsibility.trim()}
                >
                  {agentSaveState === 'saving' ? 'Saving…' : 'Save'}
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
              <div>
                <p className="font-semibold uppercase tracking-[0.2em] text-slate-400">Trace</p>
                <p className="text-sm text-white/90">
                  {agentMeta.trace?.enabled ? `Enabled (${agentMeta.trace.trace_id ?? 'pending'})` : 'Disabled'}
                </p>
              </div>
              <p className="text-[11px] text-slate-400">
                Drag the right connector to an application or another agent to define how this agent should manage it.
              </p>
            </div>
          )}
        </div>
      ) : null}
      {showAgentEditor ? (
        <div
          className="border-t border-white/5 bg-slate-950/60 px-4 py-2 text-center text-[11px] text-slate-400"
          data-tile-drag-ignore="true"
        >
          Save this agent&rsquo;s role and responsibility to keep it pinned, but the session preview remains available below.
        </div>
      ) : null}
      <section
        className={cn(
          'relative flex flex-1 flex-col gap-3 overflow-hidden bg-slate-950/60 transition-opacity',
          isInteractive ? 'pointer-events-auto select-text' : 'pointer-events-none opacity-[0.98] select-none',
          dragging && 'pointer-events-none',
        )}
        data-tile-drag-ignore="true"
      >
        {!isInteractive ? (
          <div
            className="pointer-events-auto absolute inset-0 z-10"
            onPointerEnter={() => setTerminalHover(true)}
            onPointerLeave={() => setTerminalHover(false)}
            onClick={(event) => {
              // Let clicks fall through so users can still double-click to enter interact mode
              event.stopPropagation();
              handleToggleInteractive();
            }}
          />
        ) : null}
        <ApplicationTile
          tileId={tile.id}
          privateBeachId={privateBeachId}
          managerUrl={managerBaseUrl}
          sessionMeta={tile.sessionMeta ?? null}
          onSessionMetaChange={handleMetaChange}
          onViewportMetricsChange={handleViewportMetricsChange}
          traceContext={
            isAgent && agentMeta.trace?.enabled
              ? {
                  traceId: agentMeta.trace?.trace_id ?? null,
                }
              : undefined
          }
        />
      </section>
      <button
        type="button"
        className="absolute bottom-3 right-3 z-10 h-5 w-5 cursor-nwse-resize rounded-md border border-sky-400/40 bg-[radial-gradient(circle_at_top_left,rgba(56,189,248,0.6),rgba(56,189,248,0.05))] text-transparent transition hover:border-sky-400/60 hover:shadow-[0_0_12px_rgba(56,189,248,0.45)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60"
        aria-label="Resize tile"
        onPointerDown={handleResizePointerDown}
        onPointerMove={handleResizePointerMove}
        onPointerUp={handleResizePointerUp}
        onPointerCancel={handleResizePointerCancel}
        onDoubleClick={handleResizeDoubleClick}
        data-tile-drag-ignore="true"
        data-tile-resize-handle="true"
      />
      {HANDLE_DEFS.map(({ key, pos, style }) => (
        <Handle
          key={`target-${key}`}
          type="target"
          position={pos}
          id={`target-${key}`}
          className={TARGET_HANDLE_CLASS}
          style={style}
          onPointerDown={(event) => event.stopPropagation()}
        />
      ))}
      {isAgent
        ? HANDLE_DEFS.map(({ key, pos, style }) => (
            <Handle
              key={`source-${key}`}
              type="source"
              position={pos}
              id={`source-${key}`}
              className={SOURCE_HANDLE_CLASS}
              style={style}
              onPointerDown={(event) => event.stopPropagation()}
            />
          ))
        : null}
    </article>
  );
}

function shallowEqual(a: unknown, b: unknown) {
  if (Object.is(a, b)) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || !a || !b) return false;
  const ak = Object.keys(a as Record<string, unknown>);
  const bk = Object.keys(b as Record<string, unknown>);
  if (ak.length !== bk.length) return false;
  for (const k of ak) {
    // @ts-expect-error dynamic access
    if (!Object.is(a[k], b[k])) return false;
  }
  return true;
}

function propsAreEqual(prev: Props, next: Props) {
  // Avoid re-render thrash during drag; re-render when these change.
  if (prev.dragging !== next.dragging) return false;
  const pd = prev.data;
  const nd = next.data;
  if (pd.tile.id !== nd.tile.id) return false;
  if (pd.orderIndex !== nd.orderIndex) return false;
  if (pd.isActive !== nd.isActive) return false;
  if (pd.isResizing !== nd.isResizing) return false;
  if ((pd as any).isInteractive !== (nd as any).isInteractive) return false;
  if (pd.privateBeachId !== nd.privateBeachId) return false;
  if (pd.managerUrl !== nd.managerUrl) return false;
  if (pd.rewriteEnabled !== nd.rewriteEnabled) return false;
  // Allow changes in session meta or viewport to propagate when present.
  if (!shallowEqual(pd.tile.sessionMeta ?? null, nd.tile.sessionMeta ?? null)) return false;
  if (!shallowEqual(pd.tile.agentMeta ?? null, nd.tile.agentMeta ?? null)) return false;
  // Size changes matter for layout; re-render if changed.
  if (pd.tile.size.width !== nd.tile.size.width || pd.tile.size.height !== nd.tile.size.height) return false;
  return true;
}

export const TileFlowNode = memo(TileFlowNodeImpl, propsAreEqual);
