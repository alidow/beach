import { useCallback, useEffect, useMemo, useState } from 'react';
import dynamic from 'next/dynamic';
import type { Layout } from 'react-grid-layout';
import type { SessionSummary, BeachLayoutItem, SessionRole, ControllerPairing } from '../lib/api';
import type { AssignmentEdge } from '../lib/assignments';
import { pairingStatusDisplay, formatCadenceLabel } from '../lib/pairings';
import { debugLog } from '../lib/debug';
import { SessionTerminalPreview } from './SessionTerminalPreview';
import type { HostResizeControlState } from './SessionTerminalPreviewClient';
import { Badge } from './ui/badge';
import { Button } from './ui/button';

const AutoGrid = dynamic(() => import('./AutoGrid'), {
  ssr: false,
  loading: () => <div className="h-[520px] rounded-xl border border-border bg-card shadow-sm" />,
});

const COLS = 12;
const DEFAULT_W = 4;
const DEFAULT_H = 6;

type LayoutCache = Record<string, Layout>;
type ResizeHandleAxis = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw';
type LayoutSnapshot = BeachLayoutItem;
const RESIZE_HANDLE_LABELS: Record<ResizeHandleAxis, string> = {
  n: 'Resize top edge',
  s: 'Resize bottom edge',
  e: 'Resize right edge',
  w: 'Resize left edge',
  ne: 'Resize top-right corner',
  nw: 'Resize top-left corner',
  se: 'Resize bottom-right corner',
  sw: 'Resize bottom-left corner',
};

type Props = {
  tiles: SessionSummary[];
  onRemove: (sessionId: string) => void;
  onSelect: (s: SessionSummary) => void;
  viewerToken: string | null;
  managerUrl: string;
  preset?: 'grid2x2' | 'onePlusThree' | 'focus';
  savedLayout?: BeachLayoutItem[];
  onLayoutPersist?: (layout: BeachLayoutItem[]) => void;
  roles: Map<string, SessionRole>;
  assignmentsByAgent: Map<string, AssignmentEdge[]>;
  assignmentsByApplication: Map<string, ControllerPairing[]>;
  onRequestRoleChange: (session: SessionSummary, role: SessionRole) => void;
  onOpenAssignment: (pairing: ControllerPairing) => void;
};

function presetPositions(preset: Props['preset'], count: number) {
  if (preset === 'focus') {
    return Array.from({ length: count }).map((_, idx) => ({
      x: 0,
      y: idx * DEFAULT_H,
      w: COLS,
      h: DEFAULT_H,
    }));
  }
  if (preset === 'onePlusThree') {
    const positions: Array<{ x: number; y: number; w: number; h: number }> = [];
    positions.push({ x: 0, y: 0, w: COLS, h: DEFAULT_H });
    let row = DEFAULT_H;
    for (let i = 1; i < count; i += 1) {
      const colIndex = (i - 1) % 3;
      const x = colIndex * 4;
      positions.push({
        x,
        y: row,
        w: 4,
        h: DEFAULT_H,
      });
      if (colIndex === 2) {
        row += DEFAULT_H;
      }
    }
    return positions;
  }
  const positions: Array<{ x: number; y: number; w: number; h: number }> = [];
  let y = 0;
  for (let i = 0; i < count; i += 1) {
    const x = (i % 3) * DEFAULT_W;
    positions.push({ x, y, w: DEFAULT_W, h: DEFAULT_H });
    if ((i + 1) % 3 === 0) {
      y += DEFAULT_H;
    }
  }
  return positions;
}

function nextPosition(existing: Layout[]) {
  if (existing.length === 0) {
    return { x: 0, y: 0, w: DEFAULT_W, h: DEFAULT_H };
  }
  const maxY = existing.reduce((acc, item) => Math.max(acc, item.y + item.h), 0);
  return { x: 0, y: maxY, w: DEFAULT_W, h: DEFAULT_H };
}

function ensureLayout(
  cache: LayoutCache,
  saved: LayoutSnapshot[] | undefined,
  tiles: SessionSummary[],
  preset: Props['preset'],
): Layout[] {
  const items: Layout[] = [];
  const taken = new Set<string>();
  const orderedTiles = tiles.slice();
  const savedMap = new Map<string, LayoutSnapshot>();

  saved?.forEach((item) => {
    const w = Math.min(COLS, Math.max(3, Math.floor(item.w)));
    const h = Math.max(4, Math.floor(item.h));
    const x = Math.max(0, Math.min(Math.floor(item.x), COLS - w));
    const y = Math.max(0, Math.floor(item.y));
    savedMap.set(item.id, { id: item.id, x, y, w, h });
  });

  const basePositions = presetPositions(preset, orderedTiles.length);

  orderedTiles.forEach((session, index) => {
    const id = session.session_id;
    const cached = cache[id];
    if (cached) {
      items.push({
        i: id,
        x: cached.x,
        y: cached.y,
        w: cached.w,
        h: cached.h,
        minW: cached.minW ?? 3,
        minH: cached.minH ?? 4,
      });
      taken.add(id);
      return;
    }
    const savedItem = savedMap.get(id);
    if (savedItem) {
      items.push({
        i: id,
        x: savedItem.x,
        y: savedItem.y,
        w: savedItem.w,
        h: savedItem.h,
        minW: 3,
        minH: 4,
      });
      taken.add(id);
      return;
    }
    const base = basePositions[index] ?? nextPosition(items);
    items.push({
      i: id,
      x: base.x,
      y: base.y,
      w: base.w,
      h: base.h,
      minW: 3,
      minH: 4,
    });
    taken.add(id);
  });

  return items;
}

export default function TileCanvas({
  tiles,
  onRemove,
  onSelect,
  viewerToken,
  managerUrl,
  preset = 'grid2x2',
  savedLayout,
  onLayoutPersist,
  roles,
  assignmentsByAgent,
  assignmentsByApplication,
  onRequestRoleChange,
  onOpenAssignment,
}: Props) {
  const [cache, setCache] = useState<LayoutCache>({});
  const [expanded, setExpanded] = useState<SessionSummary | null>(null);
  const [isClient, setIsClient] = useState(false);
  const [collapsedAssignments, setCollapsedAssignments] = useState<Record<string, boolean>>({});
  const [resizeControls, setResizeControls] = useState<Record<string, HostResizeControlState>>({});

  useEffect(() => {
    setIsClient(true);
  }, []);

  useEffect(() => {
    if (expanded && !tiles.some((t) => t.session_id === expanded.session_id)) {
      setExpanded(null);
    }
  }, [tiles, expanded]);

  useEffect(() => {
    if (!expanded) return;
    const match = tiles.find((t) => t.session_id === expanded.session_id);
    if (match && match !== expanded) {
      setExpanded(match);
    }
  }, [tiles, expanded]);

  const handleHostResizeStateChange = useCallback(
    (sessionId: string, state: HostResizeControlState | null) => {
      setResizeControls((prev) => {
        if (!state) {
          if (!(sessionId in prev)) {
            return prev;
          }
          const next = { ...prev };
          delete next[sessionId];
          return next;
        }
        return { ...prev, [sessionId]: state };
      });
    },
    [],
  );

  const layout = useMemo(
    () => ensureLayout(cache, savedLayout, tiles, preset),
    [cache, savedLayout, tiles, preset],
  );

  const handleLayoutChange = useCallback(
    (next: Layout[]) => {
      debugLog('tile-layout', 'layout change', {
        tileCount: next.length,
        layout: next.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })),
      });
      const nextCache: LayoutCache = { ...cache };
      next.forEach((item) => {
        nextCache[item.i] = {
          ...item,
          minW: item.minW ?? 3,
          minH: item.minH ?? 4,
        };
      });
      setCache(nextCache);
    },
    [cache],
  );

  const tileOrder = useMemo(() => tiles.map((t) => t.session_id), [tiles]);

  const snapshotLayout = useCallback(
    (next: Layout[]): BeachLayoutItem[] => {
      if (tileOrder.length === 0) return [];
      const allowed = new Set(tileOrder);
      const byId = new Map<string, BeachLayoutItem>();
      next.forEach((item) => {
        if (!allowed.has(item.i)) return;
        const w = Math.min(COLS, Math.max(3, Math.floor(item.w)));
        const h = Math.max(4, Math.floor(item.h));
        const x = Math.max(0, Math.min(Math.floor(item.x), COLS - w));
        const y = Math.max(0, Math.floor(item.y));
        byId.set(item.i, { id: item.i, x, y, w, h });
      });
      return tileOrder
        .map((id) => byId.get(id))
        .filter((entry): entry is BeachLayoutItem => Boolean(entry));
    },
    [tileOrder],
  );

  const handleLayoutCommit = useCallback(
    (next: Layout[], reason: 'drag-stop' | 'resize-stop') => {
      debugLog('tile-layout', 'layout commit', {
        reason,
        tileCount: next.length,
        layout: next.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })),
      });
      if (!onLayoutPersist) return;
      const snapshot = snapshotLayout(next);
      debugLog('tile-layout', 'persist snapshot', {
        reason,
        snapshot,
      });
      onLayoutPersist(snapshot);
    },
    [onLayoutPersist, snapshotLayout],
  );

  const handleDragStop = useCallback(
    (next: Layout[]) => {
      handleLayoutCommit(next, 'drag-stop');
    },
    [handleLayoutCommit],
  );

  const handleResizeStop = useCallback(
    (next: Layout[]) => {
      handleLayoutCommit(next, 'resize-stop');
    },
    [handleLayoutCommit],
  );

  const renderResizeHandle = useCallback((axis: string) => {
    const key = axis as ResizeHandleAxis;
    const label = RESIZE_HANDLE_LABELS[key] ?? 'Resize';
    return (
      <span
        className={`react-resizable-handle grid-resize-handle grid-resize-handle-${axis}`}
        aria-label={label}
        data-axis={axis}
      />
    );
  }, []);

  const toggleAssignments = useCallback((sessionId: string) => {
    setCollapsedAssignments((prev) => {
      const next = { ...prev };
      const current = prev[sessionId] ?? true;
      next[sessionId] = !current;
      return next;
    });
  }, []);

  if (!isClient) {
    return <div className="h-[520px] rounded-xl border border-border bg-card shadow-sm" />;
  }

  return (
    <div className="relative">
      <AutoGrid
        layout={layout}
        cols={COLS}
        rowHeight={110}
        margin={[16, 16]}
        containerPadding={[8, 8]}
        compactType={null}
        preventCollision={false}
        draggableHandle=".session-tile-drag-grip"
        draggableCancel=".session-tile-actions"
        resizeHandle={renderResizeHandle}
        resizeHandles={['n', 's', 'e', 'w', 'ne', 'nw', 'se', 'sw']}
        onDragStop={handleDragStop}
        onResizeStop={handleResizeStop}
        onLayoutChange={handleLayoutChange}
      >
        {tiles.map((session) => {
          const role = roles.get(session.session_id) ?? 'application';
          const isAgent = role === 'agent';
          const agentAssignments = assignmentsByAgent.get(session.session_id) ?? [];
          const controllers = assignmentsByApplication.get(session.session_id) ?? [];
          const collapsed = collapsedAssignments[session.session_id] ?? true;
          const resizeControl = resizeControls[session.session_id];
          const resizeColsLabel =
            resizeControl && resizeControl.viewportCols > 0
              ? String(resizeControl.viewportCols)
              : 'auto';
          const resizeButtonTitle = resizeControl
            ? `Resize host terminal to ${resizeColsLabel}×${resizeControl.viewportRows}`
            : 'Resize host terminal';

          return (
            <div
              key={session.session_id}
              className={`flex h-full flex-col overflow-hidden rounded-xl border bg-card text-card-foreground shadow-sm transition-shadow ${
                isAgent && agentAssignments.length > 0 ? 'border-primary/60' : 'border-border'
              }`}
              data-session-id={session.session_id}
            >
              <div className="flex items-center justify-between border-b border-border bg-muted/60 px-3 py-2 backdrop-blur dark:bg-muted/30">
                <div
                  className="session-tile-drag-grip flex cursor-grab items-center gap-2 text-xs text-muted-foreground active:cursor-grabbing"
                  role="button"
                  tabIndex={0}
                >
                  <span className="rounded border border-border/60 bg-background/80 px-1 font-mono text-[11px] tracking-tight">
                    {session.session_id.slice(0, 8)}
                  </span>
                  <Badge variant="muted">{session.harness_type}</Badge>
                  <Badge variant={session.last_health?.degraded ? 'warning' : 'success'}>
                    {session.last_health?.degraded ? 'degraded' : 'healthy'}
                  </Badge>
                  <Badge variant="muted">{session.pending_actions}/{session.pending_unacked}</Badge>
                  <Badge variant={isAgent ? 'default' : 'outline'}>
                    {role === 'agent' ? 'Agent' : 'Application'}
                  </Badge>
                </div>
                <div className="session-tile-actions flex items-center gap-2">
                  <Button size="sm" variant="ghost" onClick={() => onSelect(session)}>
                    Details
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setExpanded(session)}>
                    Expand ⤢
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={!resizeControl?.canResize}
                    onClick={() => {
                      if (!resizeControl?.canResize) {
                        return;
                      }
                      resizeControl.trigger();
                    }}
                    title={resizeButtonTitle}
                    aria-disabled={!resizeControl?.canResize}
                  >
                    <span className="mr-1 inline-flex h-3 w-3 items-center justify-center">
                      <svg viewBox="0 0 12 12" className="h-3 w-3" fill="none" aria-hidden>
                        <rect x="2.1" y="2.1" width="7.8" height="7.8" rx="1.5" stroke="currentColor" strokeWidth="1" />
                        <path d="M3.6 6h4.8" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
                        <path d="M6 3.6v4.8" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
                      </svg>
                    </span>
                    Resize host terminal
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => onRemove(session.session_id)}>
                    Remove
                  </Button>
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => onRequestRoleChange(session, role === 'agent' ? 'application' : 'agent')}
                  >
                    {role === 'agent' ? 'Set as Application' : 'Set as Agent'}
                  </Button>
                </div>
              </div>
              <div className="relative flex min-h-0 flex-1 bg-neutral-900">
                <SessionTerminalPreview
                  sessionId={session.session_id}
                  privateBeachId={session.private_beach_id}
                  managerUrl={managerUrl}
                  token={viewerToken}
                  harnessType={session.harness_type}
                  className="w-full"
                  onHostResizeStateChange={handleHostResizeStateChange}
                />
              </div>
              <div className="space-y-2 border-t border-border px-3 py-2">
                <div className="flex items-center justify-between">
                  <div className="text-[11px] text-muted-foreground">{session.location_hint || '—'}</div>
                  {controllers.length > 0 && (
                    <div className="flex items-center gap-1 text-[11px] text-muted-foreground">
                      {controllers.map((pairing) => (
                        <Badge
                          key={`${pairing.controller_session_id}|${pairing.child_session_id}`}
                          variant="muted"
                        >
                          {pairing.controller_session_id.slice(0, 6)}
                        </Badge>
                      ))}
                    </div>
                  )}
                </div>
                {isAgent && (
                  <div>
                    <button
                      type="button"
                      className="flex w-full items-center justify-between rounded border border-border/70 bg-muted/40 px-2 py-1 text-[11px] text-muted-foreground transition hover:bg-muted"
                      onClick={() => toggleAssignments(session.session_id)}
                    >
                      <span>
                        {agentAssignments.length === 0
                          ? 'No applications assigned'
                          : `${agentAssignments.length} assignment${agentAssignments.length === 1 ? '' : 's'}`}
                      </span>
                      <span>{collapsed ? 'Show ▾' : 'Hide ▴'}</span>
                    </button>
                    {!collapsed && (
                      <div className="mt-2 flex flex-wrap gap-2">
                        {agentAssignments.length === 0 ? (
                          <div className="text-[11px] text-muted-foreground">
                            Assign applications from the explorer.
                          </div>
                        ) : (
                          agentAssignments.map((edge) => {
                            const status = pairingStatusDisplay(edge.pairing);
                            const cadence = formatCadenceLabel(edge.pairing.update_cadence);
                            const label = edge.application
                              ? edge.application.session_id.slice(0, 8)
                              : edge.pairing.child_session_id.slice(0, 8);
                            return (
                              <button
                                type="button"
                                key={
                                  edge.pairing.pairing_id ??
                                  `${edge.pairing.controller_session_id}|${edge.pairing.child_session_id}`
                                }
                                className="flex min-w-[140px] flex-col gap-1 rounded border border-border/60 bg-background/80 px-2 py-2 text-left text-[11px] shadow-sm hover:border-primary"
                                onClick={() => onOpenAssignment(edge.pairing)}
                              >
                                <span className="font-mono text-xs text-foreground">{label}</span>
                                <div className="flex flex-wrap gap-1">
                                  <Badge variant={status.variant}>{status.label}</Badge>
                                  <Badge variant="muted">{cadence}</Badge>
                                </div>
                              </button>
                            );
                          })
                        )}
                      </div>
                    )}
                  </div>
                )}
                {!isAgent && controllers.length === 0 && (
                  <div className="text-[11px] text-muted-foreground">
                    Unassigned application — drag it onto an agent in the explorer to connect.
                  </div>
                )}
              </div>
            </div>
          );
        })}
      </AutoGrid>
      {tiles.length === 0 && (
        <div className="flex h-80 items-center justify-center rounded-xl border border-dashed border-border/70 text-sm text-muted-foreground">
          Add sessions from the explorer to build your dashboard.
        </div>
      )}
      {expanded && (
        <div className="fixed inset-0 z-50 flex flex-col bg-background/95 text-foreground backdrop-blur dark:bg-black/80">
          <div className="flex items-center justify-between border-b border-border/40 px-6 py-4">
            <div className="flex flex-wrap items-center gap-3">
              <span className="rounded border border-border/50 bg-card/80 px-2 py-1 font-mono text-sm text-card-foreground">
                {expanded.session_id}
              </span>
              <span className="text-xs uppercase tracking-wide text-muted-foreground">
                {expanded.harness_type}
              </span>
              <span className="text-xs text-muted-foreground">{expanded.location_hint || '—'}</span>
              <Badge variant={roles.get(expanded.session_id) === 'agent' ? 'default' : 'outline'}>
                {roles.get(expanded.session_id) === 'agent' ? 'Agent' : 'Application'}
              </Badge>
            </div>
            <div className="flex items-center gap-2">
              <Button
                size="sm"
                variant="outline"
                onClick={() =>
                  onRequestRoleChange(
                    expanded,
                    roles.get(expanded.session_id) === 'agent' ? 'application' : 'agent',
                  )
                }
              >
                {roles.get(expanded.session_id) === 'agent' ? 'Set as Application' : 'Set as Agent'}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setExpanded(null)}>
                Close
              </Button>
            </div>
          </div>
          <div className="flex-1 overflow-hidden">
            <SessionTerminalPreview
              sessionId={expanded.session_id}
              privateBeachId={expanded.private_beach_id}
              managerUrl={managerUrl}
              token={viewerToken}
              variant="full"
              harnessType={expanded.harness_type}
              className="h-full w-full"
            />
          </div>
        </div>
      )}
    </div>
  );
}
