import { useCallback, useEffect, useMemo, useState } from 'react';
import dynamic from 'next/dynamic';
import type { Layout } from 'react-grid-layout';
import { SessionSummary, acquireController, emergencyStop, releaseController } from '../lib/api';
import { Badge } from './ui/badge';
import { Button } from './ui/button';
import { SessionTerminalPreview } from './SessionTerminalPreview';

type Props = {
  tiles: SessionSummary[];
  onRemove: (sessionId: string) => void;
  onSelect: (s: SessionSummary) => void;
  token: string | null;
  managerUrl: string;
  refresh: () => Promise<void>;
  preset?: 'grid2x2' | 'onePlusThree' | 'focus';
};

const AutoGrid = dynamic(() => import('./AutoGrid'), {
  ssr: false,
  loading: () => <div className="h-[520px] rounded-xl border border-border bg-card shadow-sm" />,
});

const COLS = 12;
const DEFAULT_W = 4;
const DEFAULT_H = 6;
type LayoutCache = Record<string, Layout>;
type ResizeHandleAxis = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw';
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

function ensureLayout(cache: LayoutCache, tiles: SessionSummary[], preset: Props['preset']): Layout[] {
  const items: Layout[] = [];
  const taken = new Set<string>();
  const orderedTiles = tiles.slice();

  const basePositions = presetPositions(preset, orderedTiles.length);

  orderedTiles.forEach((session, index) => {
    const id = session.session_id;
    const cached = cache[id];
    if (cached) {
      items.push({ ...cached, i: id });
      taken.add(id);
      return;
    }
    const base = basePositions[index] ?? nextPosition(items);
    const layout: Layout = {
      i: id,
      x: base.x,
      y: base.y,
      w: base.w,
      h: base.h,
      minW: 3,
      minH: 4,
    };
    items.push(layout);
    taken.add(id);
  });

  return items;
}

type Position = { x: number; y: number; w: number; h: number };

function presetPositions(preset: Props['preset'], count: number): Position[] {
  if (preset === 'focus') {
    return Array.from({ length: count }).map((_, idx) => ({
      x: 0,
      y: idx * DEFAULT_H,
      w: COLS,
      h: DEFAULT_H,
    }));
  }
  if (preset === 'onePlusThree') {
    const positions: Position[] = [];
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
  // default grid2x2
  const positions: Position[] = [];
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

function nextPosition(existing: Layout[]): Position {
  if (existing.length === 0) {
    return { x: 0, y: 0, w: DEFAULT_W, h: DEFAULT_H };
  }
  const maxY = existing.reduce((acc, item) => Math.max(acc, item.y + item.h), 0);
  return { x: 0, y: maxY, w: DEFAULT_W, h: DEFAULT_H };
}

export default function TileCanvas({ tiles, onRemove, onSelect, token, managerUrl, refresh, preset = 'grid2x2' }: Props) {
  const [cache, setCache] = useState<LayoutCache>({});
  const [expanded, setExpanded] = useState<SessionSummary | null>(null);
  const [isClient, setIsClient] = useState(false);

  const renderNow = Date.now();

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

  const layout = useMemo(() => ensureLayout(cache, tiles, preset), [cache, tiles, preset]);

  const handleLayoutChange = (next: Layout[]) => {
    const nextCache: LayoutCache = { ...cache };
    next.forEach((item) => {
      nextCache[item.i] = { ...item };
    });
    setCache(nextCache);
  };

  const handleAcquire = async (sessionId: string) => {
    console.info('[tile] acquire controller', { sessionId, managerUrl, tokenPresent: Boolean(token && token.trim().length > 0) });
    if (!token || token.trim().length === 0) return;
    await acquireController(sessionId, 30000, token, managerUrl).catch(() => {});
    await refresh();
  };

  const handleRelease = async (sessionId: string, controllerToken: string | null | undefined) => {
    if (!controllerToken) return;
    console.info('[tile] release controller', { sessionId, managerUrl, controllerToken: controllerToken.slice(0, 4) + '…' });
    if (!token || token.trim().length === 0) return;
    await releaseController(sessionId, controllerToken, token, managerUrl).catch(() => {});
    await refresh();
  };

  const handleStop = async (sessionId: string) => {
    if (!confirm('Emergency stop?')) return;
    console.warn('[tile] emergency stop', { sessionId, managerUrl });
    if (!token || token.trim().length === 0) return;
    await emergencyStop(sessionId, token, managerUrl).catch(() => {});
    await refresh();
  };

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
        compactType="vertical"
        preventCollision={false}
        draggableHandle=".session-tile-drag-grip"
        draggableCancel=".session-tile-actions"
        resizeHandle={renderResizeHandle}
        resizeHandles={['n', 's', 'e', 'w', 'ne', 'nw', 'se', 'sw']}
        onLayoutChange={handleLayoutChange}
      >
        {tiles.map((s) => {
          const now = renderNow;
          const expires = s.controller_expires_at_ms || 0;
          const remain = Math.max(0, expires - now);
          const countdown = s.controller_token ? `${Math.floor(remain / 1000)}s` : '';
          return (
            <div key={s.session_id} className="flex h-full flex-col overflow-hidden rounded-xl border border-border bg-card text-card-foreground shadow-sm">
              <div className="flex items-center justify-between border-b border-border bg-muted/60 px-3 py-2 backdrop-blur dark:bg-muted/30">
                <div className="session-tile-drag-grip flex cursor-grab items-center gap-2 text-xs text-muted-foreground active:cursor-grabbing" role="button" tabIndex={0}>
                  <span className="rounded border border-border/60 bg-background/80 px-1 font-mono text-[11px] tracking-tight">{s.session_id.slice(0, 8)}</span>
                  <Badge variant="muted">{s.harness_type}</Badge>
                  <Badge variant={s.last_health?.degraded ? 'warning' : 'success'}>{s.last_health?.degraded ? 'degraded' : 'healthy'}</Badge>
                  <Badge variant="muted">{s.pending_actions}/{s.pending_unacked}</Badge>
                  {s.controller_token && <Badge variant="muted">{countdown}</Badge>}
                </div>
                <div className="session-tile-actions flex items-center gap-2">
                  <Button size="sm" variant="ghost" onClick={() => onSelect(s)}>Details</Button>
                  <Button size="sm" variant="ghost" onClick={() => setExpanded(s)}>Expand ⤢</Button>
                  <Button size="sm" variant="ghost" onClick={() => onRemove(s.session_id)}>Remove</Button>
                </div>
              </div>
              <div className="relative flex min-h-0 flex-1 bg-neutral-900">
                <SessionTerminalPreview sessionId={s.session_id} managerUrl={managerUrl} token={token} className="w-full" />
              </div>
              <div className="border-t border-border px-3 py-2">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <Button size="sm" onClick={() => handleAcquire(s.session_id)} disabled={!token || token.trim().length === 0}>Acquire</Button>
                    <Button size="sm" variant="outline" onClick={() => handleRelease(s.session_id, s.controller_token)} disabled={!token || token.trim().length === 0}>
                      Release
                    </Button>
                    <Button size="sm" variant="danger" onClick={() => handleStop(s.session_id)} disabled={!token || token.trim().length === 0}>
                      Stop
                    </Button>
                  </div>
                  <div className="text-[11px] text-muted-foreground">{s.location_hint || '—'}</div>
                </div>
              </div>
            </div>
          );
        })}
      </AutoGrid>
      {tiles.length === 0 && (
        <div className="flex h-80 items-center justify-center rounded-xl border border-dashed border-border/70 text-sm text-muted-foreground">
          Add sessions from the sidebar to build your dashboard.
        </div>
      )}
      {expanded && (
        <div className="fixed inset-0 z-50 flex flex-col bg-background/95 text-foreground backdrop-blur dark:bg-black/80">
          <div className="flex items-center justify-between border-b border-border/40 px-6 py-4">
            <div className="flex flex-wrap items-center gap-3">
              <span className="rounded border border-border/50 bg-card/80 px-2 py-1 font-mono text-sm text-card-foreground">{expanded.session_id}</span>
              <span className="text-xs uppercase tracking-wide text-muted-foreground">{expanded.harness_type}</span>
              <span className="text-xs text-muted-foreground">{expanded.location_hint || '—'}</span>
              {expanded.controller_token && (
                <span className="rounded-full bg-emerald-500/20 px-3 py-1 text-xs text-emerald-900 dark:text-emerald-100">
                  Lease active
                </span>
              )}
            </div>
            <div className="flex items-center gap-2">
              <Button size="sm" variant="outline" onClick={() => handleAcquire(expanded.session_id)} disabled={!token || token.trim().length === 0}>
                Acquire
              </Button>
              <Button
                size="sm"
                variant="outline"
                onClick={() => handleRelease(expanded.session_id, expanded.controller_token)}
                disabled={!token || token.trim().length === 0}
              >
                Release
              </Button>
              <Button size="sm" variant="danger" onClick={() => handleStop(expanded.session_id)} disabled={!token || token.trim().length === 0}>
                Stop
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setExpanded(null)}>Close</Button>
            </div>
          </div>
          <div className="flex-1 overflow-hidden">
            <SessionTerminalPreview sessionId={expanded.session_id} managerUrl={managerUrl} token={token} variant="full" className="h-full w-full" />
          </div>
        </div>
      )}
    </div>
  );
}
