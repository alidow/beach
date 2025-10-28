import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
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

const DEFAULT_COLS = 12;
const DEFAULT_W = 3;
const DEFAULT_H = 3;
const MIN_W = 2;
const MIN_H = 2;
const UNLOCKED_MAX_W = 3;
const UNLOCKED_MAX_H = 3;
const TARGET_TILE_WIDTH = 360;
const MAX_UNLOCKED_ZOOM = 1;
const MIN_ZOOM = 0.2;
const DEFAULT_ZOOM = 0.45;
const DEFAULT_HOST_COLS = 80;
const DEFAULT_HOST_ROWS = 24;
const TERMINAL_PADDING_X = 48;
const TERMINAL_PADDING_Y = 56;
const BASE_FONT_SIZE = 14;
const BASE_CELL_WIDTH = 8;
const BASE_LINE_HEIGHT = Math.round(BASE_FONT_SIZE * 1.4);
const ZOOM_EPSILON = 0.02;
const UNLOCKED_MEASUREMENT_LIMIT = TARGET_TILE_WIDTH * 1.5;

type LayoutCache = Record<string, Layout>;

type TileMeasurements = {
  width: number;
  height: number;
};

type TileViewState = {
  zoom: number;
  locked: boolean;
  toolbarPinned: boolean;
  measurements: TileMeasurements | null;
  hostCols: number | null;
  hostRows: number | null;
  viewportCols: number | null;
  viewportRows: number | null;
  lastLayout: { w: number; h: number } | null;
};

type TileStateMap = Record<string, TileViewState>;

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

function clampZoom(value: number | undefined): number {
  if (!Number.isFinite(value ?? Number.NaN)) {
    return DEFAULT_ZOOM;
  }
  return Math.min(MAX_UNLOCKED_ZOOM, Math.max(MIN_ZOOM, Number(value)));
}

function estimateHostSize(cols: number | null, rows: number | null) {
  const c = cols && cols > 0 ? cols : DEFAULT_HOST_COLS;
  const r = rows && rows > 0 ? rows : DEFAULT_HOST_ROWS;
  const width = c * BASE_CELL_WIDTH + TERMINAL_PADDING_X;
  const height = r * BASE_LINE_HEIGHT + TERMINAL_PADDING_Y;
  return { width, height };
}

function computeZoomForSize(measurements: TileMeasurements | null, hostCols: number | null, hostRows: number | null) {
  if (!measurements || measurements.width <= 0 || measurements.height <= 0) {
    return DEFAULT_ZOOM;
  }
  const hostSize = estimateHostSize(hostCols, hostRows);
  const widthRatio = measurements.width / hostSize.width;
  const heightRatio = measurements.height / hostSize.height;
  const ratio = Math.min(widthRatio, heightRatio);
  return clampZoom(ratio);
}

function isSameMeasurement(a: TileMeasurements | null, b: TileMeasurements | null): boolean {
  if (!a && !b) return true;
  if (!a || !b) return false;
  return Math.abs(a.width - b.width) < 0.5 && Math.abs(a.height - b.height) < 0.5;
}

function isSameLayoutDimensions(a: { w: number; h: number } | null, b: { w: number; h: number } | null): boolean {
  if (!a && !b) return true;
  if (!a || !b) return false;
  return a.w === b.w && a.h === b.h;
}

function clampGridSize(
  w: number,
  h: number,
  state: TileViewState | undefined,
  cols: number,
  restrictUnlocked = false,
): { w: number; h: number } {
  const ensuredW = Math.max(MIN_W, Math.min(Math.round(w), cols));
  const ensuredH = Math.max(MIN_H, Math.round(h));
  if (!state) {
    return {
      w: restrictUnlocked ? Math.min(ensuredW, UNLOCKED_MAX_W) : ensuredW,
      h: restrictUnlocked ? Math.min(ensuredH, UNLOCKED_MAX_H) : ensuredH,
    };
  }
  if (state.locked) {
    return { w: ensuredW, h: ensuredH };
  }
  if (restrictUnlocked) {
    return {
      w: Math.min(ensuredW, UNLOCKED_MAX_W),
      h: Math.min(ensuredH, UNLOCKED_MAX_H),
    };
  }
  return { w: ensuredW, h: ensuredH };
}

function buildTileState(saved?: BeachLayoutItem): TileViewState {
  const locked = Boolean(saved?.locked);
  const savedMeasurement =
    saved?.widthPx && saved?.heightPx
      ? { width: saved.widthPx, height: saved.heightPx }
      : null;
  const measurement =
    !locked && savedMeasurement && savedMeasurement.width > UNLOCKED_MEASUREMENT_LIMIT
      ? null
      : savedMeasurement;
  const baselineZoom = locked ? MAX_UNLOCKED_ZOOM : clampZoom(saved?.zoom);
  const zoom =
    !locked && baselineZoom >= MAX_UNLOCKED_ZOOM - ZOOM_EPSILON ? DEFAULT_ZOOM : baselineZoom;
  const baseline: TileViewState = {
    zoom,
    locked,
    toolbarPinned: Boolean(saved?.toolbarPinned),
    measurements: measurement,
    hostCols: null,
    hostRows: null,
    viewportCols: null,
    viewportRows: null,
    lastLayout: null,
  };
  if (saved) {
    const { w, h } = clampGridSize(saved.w, saved.h, baseline, DEFAULT_COLS, true);
    baseline.lastLayout = { w, h };
  }
  return baseline;
}

function isTileCropped(hostCols: number | null, hostRows: number | null): boolean {
  const c = hostCols && hostCols > 0 ? hostCols : DEFAULT_HOST_COLS;
  const r = hostRows && hostRows > 0 ? hostRows : DEFAULT_HOST_ROWS;
  return c > 80 || r > 80;
}

function isTileStateEqual(a: TileViewState, b: TileViewState): boolean {
  return (
    a.zoom === b.zoom &&
    a.locked === b.locked &&
    a.toolbarPinned === b.toolbarPinned &&
    a.hostCols === b.hostCols &&
    a.hostRows === b.hostRows &&
    a.viewportCols === b.viewportCols &&
    a.viewportRows === b.viewportRows &&
    isSameMeasurement(a.measurements, b.measurements) &&
    isSameLayoutDimensions(a.lastLayout, b.lastLayout)
  );
}

function presetPositions(
  preset: 'grid2x2' | 'onePlusThree' | 'focus' | undefined,
  count: number,
  cols: number,
) {
  if (preset === 'focus') {
    return Array.from({ length: count }).map((_, idx) => ({
      x: 0,
      y: idx * DEFAULT_H,
      w: cols,
      h: DEFAULT_H,
    }));
  }
  if (preset === 'onePlusThree') {
    const positions: Array<{ x: number; y: number; w: number; h: number }> = [];
    positions.push({ x: 0, y: 0, w: cols, h: DEFAULT_H });
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
  saved: BeachLayoutItem[] | undefined,
  tiles: SessionSummary[],
  preset: 'grid2x2' | 'onePlusThree' | 'focus' | undefined,
  viewState: TileStateMap,
  cols: number,
): Layout[] {
  const effectiveCols = Math.max(DEFAULT_W, cols || DEFAULT_COLS);
  const items: Layout[] = [];
  const taken = new Set<string>();
  const orderedTiles = tiles.slice();
  const savedMap = new Map<string, BeachLayoutItem>();

  saved?.forEach((item) => {
    const w = Math.min(effectiveCols, Math.max(MIN_W, Math.floor(item.w)));
    const h = Math.max(MIN_H, Math.floor(item.h));
    const x = Math.max(0, Math.min(Math.floor(item.x), effectiveCols - w));
    const y = Math.max(0, Math.floor(item.y));
    savedMap.set(item.id, { ...item, x, y, w, h });
  });

  const basePositions = presetPositions(preset, orderedTiles.length, effectiveCols);

  orderedTiles.forEach((session, index) => {
    const id = session.session_id;
    const cached = cache[id];
    const state = viewState[id];
    if (cached) {
      const { w, h } = clampGridSize(cached.w, cached.h, state, effectiveCols);
      const x = Math.max(0, Math.min(cached.x, effectiveCols - w));
      items.push({
        i: id,
        x,
        y: cached.y,
        w,
        h,
        minW: MIN_W,
        minH: MIN_H,
        isResizable: state?.locked || (state ? state.zoom < MAX_UNLOCKED_ZOOM - ZOOM_EPSILON : true),
      });
      taken.add(id);
      return;
    }
    const savedItem = savedMap.get(id);
    if (savedItem) {
      const restrict = !state?.lastLayout;
      const { w, h } = clampGridSize(savedItem.w, savedItem.h, state, effectiveCols, restrict);
      const x = Math.max(0, Math.min(savedItem.x, effectiveCols - w));
      items.push({
        i: id,
        x,
        y: savedItem.y,
        w,
        h,
        minW: MIN_W,
        minH: MIN_H,
        isResizable: state?.locked || clampZoom(savedItem.zoom) < MAX_UNLOCKED_ZOOM - ZOOM_EPSILON,
      });
      taken.add(id);
      return;
    }
    const base = basePositions[index] ?? nextPosition(items);
    const restrict = !state?.lastLayout;
    const { w, h } = clampGridSize(base.w, base.h, state, effectiveCols, restrict);
    const x = Math.max(0, Math.min(base.x, effectiveCols - w));
    items.push({
      i: id,
      x,
      y: base.y,
      w,
      h,
      minW: MIN_W,
      minH: MIN_H,
      isResizable: state?.locked || !state || state.zoom < MAX_UNLOCKED_ZOOM - ZOOM_EPSILON,
    });
    taken.add(id);
  });

  return items;
}

type IconButtonProps = {
  title: string;
  onClick: () => void;
  disabled?: boolean;
  pressed?: boolean;
  ariaLabel?: string;
  children: ReactNode;
};

function IconButton({ title, onClick, disabled, pressed, ariaLabel, children }: IconButtonProps) {
  return (
    <button
      type="button"
      className={`flex h-7 w-7 items-center justify-center rounded-full border border-white/10 bg-black/25 text-muted-foreground transition hover:border-white/30 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-40 ${
        pressed ? 'border-primary/60 bg-primary/15 text-primary-foreground' : ''
      }`}
      title={title}
      aria-label={ariaLabel ?? title}
      aria-pressed={pressed}
      onClick={onClick}
      disabled={disabled}
    >
      {children}
    </button>
  );
}

function SnapIcon() {
  return (
    <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.2">
      <rect x="3" y="3" width="10" height="10" rx="2" />
      <path d="M6 6h4v4H6z" stroke="none" fill="currentColor" />
    </svg>
  );
}

function LockIcon() {
  return (
    <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.2">
      <rect x="4" y="7" width="8" height="6" rx="1.6" />
      <path d="M6 6a2 2 0 1 1 4 0v1" />
    </svg>
  );
}

function UnlockIcon() {
  return (
    <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.2">
      <rect x="4" y="7" width="8" height="6" rx="1.6" />
      <path d="M6 6c0-1.1.9-2 2-2s2 .9 2 2" />
      <path d="M10 6V5.2c0-1.3-.9-2.4-2.2-2.6A2.4 2.4 0 0 0 5.4 5" />
    </svg>
  );
}

function ExpandIcon() {
  return (
    <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.2">
      <path d="M6.2 3.2H3.2v3" strokeLinecap="round" />
      <path d="M9.8 12.8h3v-3" strokeLinecap="round" />
      <path d="M3.2 6.8l3.2-3.6" strokeLinecap="round" />
      <path d="M12.8 9.2l-3.2 3.6" strokeLinecap="round" />
    </svg>
  );
}

function InfoIcon() {
  return (
    <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.2">
      <circle cx="8" cy="8" r="5.2" />
      <path d="M8 5.2h.01" strokeLinecap="round" />
      <path d="M7.4 7.4h.8v3.2" strokeLinecap="round" />
    </svg>
  );
}

function RemoveIcon() {
  return (
    <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.2">
      <rect x="3.5" y="4.5" width="9" height="8" rx="1.4" />
      <path d="M6 3.5h4" strokeLinecap="round" />
      <path d="M5 6.5l6 6" strokeLinecap="round" />
      <path d="M11 6.5l-6 6" strokeLinecap="round" />
    </svg>
  );
}

type TileCardProps = {
  session: SessionSummary;
  role: SessionRole;
  isAgent: boolean;
  assignments: AssignmentEdge[];
  controllers: ControllerPairing[];
  collapsed: boolean;
  onToggleAssignments: () => void;
  onOpenAssignment: (pairing: ControllerPairing) => void;
  onSelect: () => void;
  onRemove: () => void;
  onToggleRole: () => void;
  onExpand: () => void;
  onSnap: () => void;
  onToggleLock: () => void;
  onToolbarToggle: () => void;
  resizeControl: HostResizeControlState | undefined;
  managerUrl: string;
  viewerToken: string | null;
  view: TileViewState;
  onMeasure: (measurement: TileMeasurements) => void;
  onViewport: (dims: {
    viewportRows: number;
    viewportCols: number;
    hostRows: number | null;
    hostCols: number | null;
  }) => void;
  onHostResizeStateChange: (sessionId: string, state: HostResizeControlState | null) => void;
};

function TileCard({
  session,
  role,
  isAgent,
  assignments,
  controllers,
  collapsed,
  onToggleAssignments,
  onOpenAssignment,
  onSelect,
  onRemove,
  onToggleRole,
  onExpand,
  onSnap,
  onToggleLock,
  onToolbarToggle,
  resizeControl,
  managerUrl,
  viewerToken,
  view,
  onMeasure,
  onViewport,
  onHostResizeStateChange,
}: TileCardProps) {
  const contentRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const element = contentRef.current;
    if (!element || typeof ResizeObserver === 'undefined') {
      return;
    }
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      const { width, height } = entry.contentRect;
      if (width > 0 && height > 0) {
        onMeasure({ width, height });
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [onMeasure]);

  const toolbarVisibleClass = view.toolbarPinned ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 group-focus-within:opacity-100';
  const zoomDisplay = view.locked ? MAX_UNLOCKED_ZOOM : clampZoom(view.zoom);
  const fontSize = Math.round(zoomDisplay * BASE_FONT_SIZE * 100) / 100;
  const zoomLabel = `${Math.round(zoomDisplay * 100)}%`;
  const cropped = isTileCropped(view.hostCols, view.hostRows);
  const resizeHint =
    resizeControl && resizeControl.canResize
      ? `Resize host to ${resizeControl.viewportCols}×${resizeControl.viewportRows}`
      : 'Host resize unavailable';

  return (
    <div
      className={`group relative flex h-full w-full flex-col overflow-hidden rounded-xl border bg-card text-card-foreground shadow-sm transition-shadow ${
        isAgent && assignments.length > 0 ? 'border-primary/60' : 'border-border'
      }`}
      data-session-id={session.session_id}
    >
      <div
        className={`pointer-events-none absolute inset-x-2 top-2 z-20 flex items-center justify-between rounded-full bg-background/80 px-3 py-1 text-[11px] font-medium text-muted-foreground shadow-sm backdrop-blur transition-opacity ${toolbarVisibleClass}`}
      >
        <button
          type="button"
          className="pointer-events-auto session-tile-drag-grip flex items-center gap-2 text-[10px] uppercase tracking-[0.36em] text-muted-foreground"
          onDoubleClick={onToolbarToggle}
        >
          <span className="rounded border border-border/60 bg-background/70 px-2 py-0.5 font-mono text-[10px] tracking-tight">
            {session.session_id.slice(0, 8)}
          </span>
        </button>
        <div className="pointer-events-auto flex items-center gap-2">
          <IconButton title="Snap to host size" ariaLabel="Snap to host size" onClick={onSnap} disabled={view.locked}>
            <SnapIcon />
          </IconButton>
          <IconButton
            title={
              view.locked
                ? 'Unlock tile (resize without touching host)'
                : 'Lock tile and resize host PTY'
            }
            ariaLabel={view.locked ? 'Unlock tile' : 'Lock tile'}
            onClick={onToggleLock}
            disabled={!view.locked && zoomDisplay < MAX_UNLOCKED_ZOOM - 0.01}
            pressed={view.locked}
          >
            {view.locked ? <LockIcon /> : <UnlockIcon />}
          </IconButton>
          <IconButton title="Remove tile" ariaLabel="Remove tile" onClick={onRemove}>
            <RemoveIcon />
          </IconButton>
          <IconButton title="Expand tile" ariaLabel="Expand tile" onClick={onExpand}>
            <ExpandIcon />
          </IconButton>
          <IconButton title="View details" ariaLabel="View details" onClick={onSelect}>
            <InfoIcon />
          </IconButton>
        </div>
      </div>
      <div className="flex-1 space-y-3 pt-9">
        <div
          ref={contentRef}
          className="relative flex min-h-0 flex-1 overflow-hidden rounded-lg border border-border/60 bg-neutral-900"
        >
          <SessionTerminalPreview
            sessionId={session.session_id}
            privateBeachId={session.private_beach_id}
            managerUrl={managerUrl}
            token={viewerToken}
            harnessType={session.harness_type}
            className="h-full w-full"
            onHostResizeStateChange={onHostResizeStateChange}
            onViewportDimensions={onViewport}
            fontSize={fontSize}
            locked={view.locked}
            cropped={cropped}
          />
        </div>
        <div className="space-y-2 border-t border-border px-3 pb-3 pt-2">
          <div className="flex items-center justify-between">
            <div className="text-[11px] text-muted-foreground">{session.location_hint || '—'}</div>
            {controllers.length > 0 && (
              <div className="flex items-center gap-1 text-[11px] text-muted-foreground">
                {controllers.map((pairing) => (
                  <Badge key={`${pairing.controller_session_id}|${pairing.child_session_id}`} variant="muted">
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
                onClick={onToggleAssignments}
              >
                <span>
                  {assignments.length === 0
                    ? 'No applications assigned'
                    : `${assignments.length} assignment${assignments.length === 1 ? '' : 's'}`}
                </span>
                <span>{collapsed ? 'Show ▾' : 'Hide ▴'}</span>
              </button>
              {!collapsed && (
                <div className="mt-2 flex flex-wrap gap-2">
                  {assignments.length === 0 ? (
                    <div className="text-[11px] text-muted-foreground">
                      Assign applications from the explorer.
                    </div>
                  ) : (
                    assignments.map((edge) => {
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
                          className="flex min-w-[140px] flex-col gap-1 rounded border border-border/60 bg-background/80 px-2 py-2 text-left text-[11px] shadow-sm transition hover:border-primary"
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
          <div className="flex items-center justify-between pt-1 text-[11px] text-muted-foreground">
            <div className="flex items-center gap-2">
              <span>{view.locked ? 'Locked' : `Zoom ${zoomLabel}`}</span>
              {resizeControl?.needsResize && (
                <span className="rounded-full border border-amber-500/40 bg-amber-500/10 px-2 py-[1px] text-amber-200">
                  Host mismatch
                </span>
              )}
            </div>
            <div className="flex items-center gap-2">
              <Button size="sm" variant="ghost" onClick={onRemove}>
                Remove
              </Button>
              <Button size="sm" variant="outline" onClick={onToggleRole}>
                {role === 'agent' ? 'Set as Application' : 'Set as Agent'}
              </Button>
            </div>
          </div>
          <div className="text-[10px] text-muted-foreground/80">
            {resizeControl?.canResize ? resizeHint : 'Waiting for host viewport…'}
          </div>
        </div>
      </div>
    </div>
  );
}

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
  const [cols, setCols] = useState(DEFAULT_COLS);
  const [gridWidth, setGridWidth] = useState<number | null>(null);
  const [tileState, setTileState] = useState<TileStateMap>(() => {
    const initial: TileStateMap = {};
    savedLayout?.forEach((item) => {
      initial[item.id] = buildTileState(item);
    });
    tiles.forEach((session) => {
      if (!initial[session.session_id]) {
        initial[session.session_id] = buildTileState();
      }
    });
    return initial;
  });

  const tileStateRef = useRef<TileStateMap>(tileState);
  const prevTileStateRef = useRef<TileStateMap>({});
  const resizeControlRef = useRef<Record<string, HostResizeControlState>>(resizeControls);
  const gridWrapperRef = useRef<HTMLDivElement | null>(null);
  const lastPersistSignatureRef = useRef<string | null>(null);

  const computeCols = useCallback((width: number) => {
    const effectiveWidth = Math.max(width, TARGET_TILE_WIDTH);
    const units = Math.max(1, Math.round(effectiveWidth / TARGET_TILE_WIDTH));
    const desired = Math.min(48, Math.max(DEFAULT_W, units * DEFAULT_W));
    return desired;
  }, []);

  useEffect(() => {
    tileStateRef.current = tileState;
  }, [tileState]);

  useEffect(() => {
    resizeControlRef.current = resizeControls;
  }, [resizeControls]);

  useEffect(() => {
    setIsClient(true);
  }, []);

  useEffect(() => {
    if (!isClient) {
      return undefined;
    }
    const applyWidth = (width: number | null) => {
      const fallback = typeof window !== 'undefined' ? window.innerWidth : TARGET_TILE_WIDTH;
      const targetWidth = width ?? fallback ?? TARGET_TILE_WIDTH;
      const nextCols = computeCols(targetWidth);
      setGridWidth(targetWidth);
      setCols((prev) => (prev === nextCols ? prev : nextCols));
    };
    const element = gridWrapperRef.current;
    if (element && typeof ResizeObserver !== 'undefined') {
      const observer = new ResizeObserver((entries) => {
        const entry = entries[0];
        if (entry) {
          applyWidth(entry.contentRect.width);
        }
      });
      observer.observe(element);
      applyWidth(element.getBoundingClientRect().width);
      return () => observer.disconnect();
    }
    const handle = () => applyWidth(window.innerWidth || TARGET_TILE_WIDTH);
    handle();
    window.addEventListener('resize', handle);
    return () => {
      window.removeEventListener('resize', handle);
    };
  }, [computeCols, isClient]);

  useEffect(() => {
    debugLog('tile-layout', 'cols update', { cols });
  }, [cols]);

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

  useEffect(() => {
    setTileState((prev) => {
      let changed = false;
      const next: TileStateMap = { ...prev };
      tiles.forEach((session) => {
        if (!next[session.session_id]) {
          next[session.session_id] = buildTileState();
          changed = true;
        }
      });
      const allowedIds = new Set(tiles.map((t) => t.session_id));
      Object.keys(next).forEach((id) => {
        if (!allowedIds.has(id)) {
          delete next[id];
          changed = true;
        }
      });
      savedLayout?.forEach((item) => {
        const current = next[item.id] ?? buildTileState();
        const merged: TileViewState = {
          ...current,
          locked: typeof item.locked === 'boolean' ? item.locked : current.locked,
          toolbarPinned:
            typeof item.toolbarPinned === 'boolean' ? item.toolbarPinned : current.toolbarPinned,
          lastLayout: null,
        };
        const initialSize = clampGridSize(item.w, item.h, merged, cols, true);
        merged.lastLayout = initialSize;
        if (item.widthPx && item.heightPx) {
          merged.measurements = { width: item.widthPx, height: item.heightPx };
        }
        if (!merged.locked) {
          merged.zoom = clampZoom(item.zoom ?? merged.zoom);
        } else {
          merged.zoom = MAX_UNLOCKED_ZOOM;
        }
        if (
          !isSameMeasurement(current.measurements, merged.measurements) ||
          current.locked !== merged.locked ||
          current.toolbarPinned !== merged.toolbarPinned ||
          current.zoom !== merged.zoom ||
          !isSameLayoutDimensions(current.lastLayout, merged.lastLayout)
        ) {
          next[item.id] = merged;
          changed = true;
        }
      });
      return changed ? next : prev;
    });
  }, [cols, savedLayout, tiles]);

  const layout = useMemo(() => {
    const adjusted = ensureLayout(cache, savedLayout, tiles, preset, tileState, cols).map((item) => {
      const state = tileState[item.i];
      return {
        ...item,
        maxW: state?.locked ? cols : cols,
        maxH: state?.locked ? cols : cols,
      };
    });
    if (typeof window !== 'undefined') {
      console.info(
        '[tile-layout] ensure',
        JSON.stringify({
          cols,
          items: adjusted.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })),
        }),
      );
    }
    debugLog('tile-layout', 'ensure layout', {
      cols,
      layout: adjusted.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })),
    });
    return adjusted;
  }, [cache, savedLayout, tiles, preset, tileState, cols]);

const layoutMap = useMemo(() => {
  const map = new Map<string, Layout>();
  layout.forEach((item) => map.set(item.i, item));
  return map;
}, [layout]);

const tileOrder = useMemo(() => tiles.map((t) => t.session_id), [tiles]);
  const clampLayoutItems = useCallback(
    (layouts: Layout[], colsValue: number): Layout[] => {
      const effectiveCols = Math.max(DEFAULT_W, colsValue || DEFAULT_COLS);
      const stateMap = tileStateRef.current;
      return layouts.map((item) => {
        const state = stateMap[item.i];
        const { w, h } = clampGridSize(item.w, item.h, state, effectiveCols);
        const x = Math.max(0, Math.min(item.x, effectiveCols - w));
        return {
          ...item,
          x,
          w,
          h,
          minW: MIN_W,
          minH: MIN_H,
        };
      });
    },
    [],
  );

  const snapshotLayout = useCallback(
    (nextLayouts: Layout[], colsValue: number): BeachLayoutItem[] => {
      if (tileOrder.length === 0) return [];
      const allowed = new Set(tileOrder);
      const byId = new Map<string, BeachLayoutItem>();
      const stateMap = tileStateRef.current;
      const effectiveCols = Math.max(DEFAULT_W, colsValue || DEFAULT_COLS);
      nextLayouts.forEach((item) => {
        if (!allowed.has(item.i)) return;
        const w = Math.min(effectiveCols, Math.max(MIN_W, Math.floor(item.w)));
        const h = Math.max(MIN_H, Math.floor(item.h));
        const x = Math.max(0, Math.min(Math.floor(item.x), effectiveCols - w));
        const y = Math.max(0, Math.floor(item.y));
        const view = stateMap[item.i];
        const entry: BeachLayoutItem = { id: item.i, x, y, w, h };
        if (view?.measurements) {
          entry.widthPx = Math.round(view.measurements.width);
          entry.heightPx = Math.round(view.measurements.height);
        }
        if (typeof view?.zoom === 'number') {
          entry.zoom = Number((view.locked ? MAX_UNLOCKED_ZOOM : view.zoom).toFixed(3));
        }
        if (typeof view?.locked === 'boolean') {
          entry.locked = view.locked;
        }
        if (typeof view?.toolbarPinned === 'boolean') {
          entry.toolbarPinned = view.toolbarPinned;
        }
        byId.set(item.i, entry);
      });
      return tileOrder
        .map((id) => byId.get(id))
        .filter((entry): entry is BeachLayoutItem => Boolean(entry));
    },
    [tileOrder],
  );

  const handleLayoutCommit = useCallback(
    (nextLayouts: Layout[], reason: 'drag-stop' | 'resize-stop' | 'state-change') => {
      const normalized = clampLayoutItems(nextLayouts, cols);
      if (typeof window !== 'undefined') {
        console.info(
          '[tile-layout] commit',
          JSON.stringify({
            reason,
            cols,
            items: normalized.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })),
          }),
        );
      }
      debugLog('tile-layout', 'layout commit', {
        reason,
        tileCount: normalized.length,
        layout: normalized.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })),
      });
      if (!onLayoutPersist) return;
      const snapshot = snapshotLayout(normalized, cols);
      onLayoutPersist(snapshot);
    },
    [clampLayoutItems, cols, onLayoutPersist, snapshotLayout],
  );

  const handleLayoutChange = useCallback(
    (nextLayouts: Layout[]) => {
      const normalized = clampLayoutItems(nextLayouts, cols);
      debugLog('tile-layout', 'layout change', {
        tileCount: normalized.length,
        layout: normalized.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })),
      });
      setCache((prev) => {
        const nextCache: LayoutCache = { ...prev };
        normalized.forEach((item) => {
          nextCache[item.i] = {
            ...item,
            minW: MIN_W,
            minH: MIN_H,
          };
        });
        return nextCache;
      });
      setTileState((prev) => {
        let changed = false;
        const nextState: TileStateMap = { ...prev };
        normalized.forEach((item) => {
          const current = nextState[item.i] ?? buildTileState();
        const dims = { w: item.w, h: item.h };
          if (!isSameLayoutDimensions(current.lastLayout, dims)) {
            nextState[item.i] = { ...current, lastLayout: dims };
            changed = true;
          }
        });
        return changed ? nextState : prev;
      });
    },
    [clampLayoutItems, cols],
  );

  const scheduleHostResize = useCallback((sessionId: string) => {
    if (typeof window === 'undefined') {
      return;
    }
    const control = resizeControlRef.current[sessionId];
    if (!control?.canResize) return;
    window.setTimeout(() => {
      const current = resizeControlRef.current[sessionId];
      if (current?.canResize) {
        current.trigger();
      }
    }, 120);
  }, []);

  useEffect(() => {
    const previous = prevTileStateRef.current;
    Object.entries(tileState).forEach(([id, state]) => {
      const before = previous[id];
      if (state.locked && (!before || !before.locked)) {
        scheduleHostResize(id);
      }
    });
    prevTileStateRef.current = tileState;
  }, [tileState, scheduleHostResize]);

  const updateTileState = useCallback(
    (sessionId: string, producer: (state: TileViewState) => TileViewState) => {
      setTileState((prev) => {
        const current = prev[sessionId] ?? buildTileState();
        let next = producer(current);
        const computedZoom = computeZoomForSize(next.measurements, next.hostCols, next.hostRows);
        if (next.locked) {
          next = { ...next, zoom: MAX_UNLOCKED_ZOOM };
        } else {
          const selected = Number.isFinite(next.zoom) ? Number(next.zoom) : computedZoom;
          next = { ...next, zoom: clampZoom(selected) };
        }
        if (
          current.zoom === next.zoom &&
          current.locked === next.locked &&
          current.toolbarPinned === next.toolbarPinned &&
          current.hostCols === next.hostCols &&
          current.hostRows === next.hostRows &&
          current.viewportCols === next.viewportCols &&
          current.viewportRows === next.viewportRows &&
          isSameMeasurement(current.measurements, next.measurements) &&
          isSameLayoutDimensions(current.lastLayout, next.lastLayout)
        ) {
          return prev;
        }
        return { ...prev, [sessionId]: next };
      });
    },
    [],
  );

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

  const handleDragStop = useCallback(
    (next: Layout[]) => {
      handleLayoutCommit(next, 'drag-stop');
    },
    [handleLayoutCommit],
  );

  const handleResizeStop = useCallback(
    (
      nextLayouts: Layout[],
      _oldItem: Layout,
      newItem: Layout,
      _placeholder: Layout,
      _event: MouseEvent,
      element: HTMLElement | undefined,
    ) => {
      const state = tileStateRef.current[newItem.i] ?? buildTileState();
      const hostSize = estimateHostSize(state.hostCols, state.hostRows);
      const widthPx = element?.offsetWidth ?? state.measurements?.width ?? 0;
      const heightPx = element?.offsetHeight ?? state.measurements?.height ?? 0;
      let adjustedLayouts = nextLayouts;

      if (element && widthPx > 0 && heightPx > 0) {
        const unitWidth = newItem.w > 0 ? widthPx / newItem.w : widthPx;
        const hostAspect = hostSize.width / hostSize.height;
        const targetHeightPx = widthPx / hostAspect;
        if (unitWidth > 0) {
          const unitHeight = newItem.h > 0 ? heightPx / newItem.h : heightPx;
          if (unitHeight > 0) {
            const targetHUnits = Math.max(MIN_H, Math.round(targetHeightPx / unitHeight));
            if (targetHUnits !== newItem.h) {
              adjustedLayouts = nextLayouts.map((item) =>
                item.i === newItem.i ? { ...item, h: targetHUnits } : item,
              );
            }
          }
        }

        if (!state.locked) {
          const zoomCandidate = computeZoomForSize({ width: widthPx, height: heightPx }, state.hostCols, state.hostRows);
          if (zoomCandidate >= MAX_UNLOCKED_ZOOM - ZOOM_EPSILON) {
            const unitWidthPx = newItem.w > 0 ? widthPx / newItem.w : widthPx;
            const unitHeightPx = newItem.h > 0 ? heightPx / newItem.h : heightPx;
            if (unitWidthPx > 0 && unitHeightPx > 0) {
              const targetWUnits = Math.max(MIN_W, Math.round(hostSize.width / unitWidthPx));
              const targetHUnits = Math.max(MIN_H, Math.round(hostSize.height / unitHeightPx));
              adjustedLayouts = nextLayouts.map((item) =>
                item.i === newItem.i ? { ...item, w: targetWUnits, h: targetHUnits } : item,
              );
            }
          }
        }
      }

      const normalized = clampLayoutItems(adjustedLayouts, cols);
      handleLayoutChange(normalized);
      handleLayoutCommit(normalized, 'resize-stop');
      updateTileState(newItem.i, (current) => ({
        ...current,
        measurements:
          widthPx > 0 && heightPx > 0
            ? { width: widthPx, height: heightPx }
            : current.measurements,
      }));
      if (state.locked) {
        scheduleHostResize(newItem.i);
      }
    },
    [clampLayoutItems, cols, handleLayoutChange, handleLayoutCommit, scheduleHostResize, updateTileState],
  );

  const renderResizeHandle = useCallback((axis: string) => {
    const label = RESIZE_HANDLE_LABELS[axis as keyof typeof RESIZE_HANDLE_LABELS] ?? 'Resize';
    return (
      <span
        className={`react-resizable-handle grid-resize-handle grid-resize-handle-${axis}`}
        aria-label={label}
        data-axis={axis}
      />
    );
  }, []);

  const handleMeasure = useCallback(
    (sessionId: string, measurement: TileMeasurements) => {
      const layoutItem = layoutMap.get(sessionId);
      const widthFromLayout =
        layoutItem && gridWidth != null && cols > 0
          ? (gridWidth / cols) * layoutItem.w
          : null;
      const normalized: TileMeasurements = {
        width: widthFromLayout && Number.isFinite(widthFromLayout) ? widthFromLayout : measurement.width,
        height: measurement.height,
      };
      updateTileState(sessionId, (state) => ({
        ...state,
        measurements: normalized,
      }));
      if (typeof window !== 'undefined') {
        console.info(
          '[tile-layout] measure',
          JSON.stringify({
            sessionId,
            width: normalized.width,
            height: normalized.height,
          }),
        );
      }
    },
    [cols, gridWidth, layoutMap, updateTileState],
  );

  const handleViewportDimensions = useCallback(
    (
      sessionId: string,
      dims: {
        viewportRows: number;
        viewportCols: number;
        hostRows: number | null;
        hostCols: number | null;
      },
    ) => {
      updateTileState(sessionId, (state) => ({
        ...state,
        viewportRows: dims.viewportRows,
        viewportCols: dims.viewportCols,
        hostRows: typeof dims.hostRows === 'number' && dims.hostRows > 0 ? dims.hostRows : state.hostRows,
        hostCols: typeof dims.hostCols === 'number' && dims.hostCols > 0 ? dims.hostCols : state.hostCols,
      }));
    },
    [updateTileState],
  );

  const handleSnap = useCallback(
    (sessionId: string) => {
      const layoutItem = layoutMap.get(sessionId);
      const state = tileStateRef.current[sessionId];
      const measurement = state?.measurements;
      if (!layoutItem || !state || !measurement) {
        updateTileState(sessionId, (current) => ({
          ...current,
          locked: false,
          zoom: DEFAULT_ZOOM,
        }));
        return;
      }
      const hostSize = estimateHostSize(state.hostCols, state.hostRows);
      const unitWidth =
        gridWidth != null && cols > 0
          ? Math.max(1, gridWidth / cols)
          : layoutItem.w > 0
            ? Math.max(1, measurement.width / layoutItem.w)
            : Math.max(1, measurement.width);
      const unitHeight =
        layoutItem.h > 0 ? Math.max(1, measurement.height / layoutItem.h) : Math.max(1, measurement.height);
      const targetWUnits = Math.max(MIN_W, Math.round(hostSize.width / unitWidth));
      const targetHUnits = Math.max(MIN_H, Math.round(hostSize.height / unitHeight));
      const nextLayouts = clampLayoutItems(
        layout.map((item) =>
          item.i === sessionId
            ? {
                ...item,
                w: Math.min(targetWUnits, cols),
                h: Math.min(targetHUnits, cols),
              }
            : item,
        ),
        cols,
      );
      handleLayoutChange(nextLayouts);
      handleLayoutCommit(nextLayouts, 'state-change');
      updateTileState(sessionId, (current) => ({
        ...current,
        locked: false,
        zoom: clampZoom(
          Math.min(targetWUnits, cols) >= cols || Math.min(targetHUnits, cols) >= cols
            ? MAX_UNLOCKED_ZOOM
            : DEFAULT_ZOOM,
        ),
      }));
    },
    [clampLayoutItems, cols, gridWidth, handleLayoutChange, handleLayoutCommit, layout, layoutMap, updateTileState],
  );

  const handleToggleLock = useCallback(
    (sessionId: string) => {
      updateTileState(sessionId, (current) => ({
        ...current,
        locked: !current.locked,
        zoom: !current.locked ? MAX_UNLOCKED_ZOOM : clampZoom(current.zoom),
      }));
    },
    [updateTileState],
  );

  const handleToolbarToggle = useCallback(
    (sessionId: string) => {
      updateTileState(sessionId, (current) => ({
        ...current,
        toolbarPinned: !current.toolbarPinned,
      }));
    },
    [updateTileState],
  );

  const toggleAssignments = useCallback((sessionId: string) => {
    setCollapsedAssignments((prev) => {
      const next = { ...prev };
      const current = prev[sessionId] ?? true;
      next[sessionId] = !current;
      return next;
    });
  }, []);

  useEffect(() => {
    if (!isClient || !onLayoutPersist || !savedLayout || savedLayout.length === 0 || layout.length === 0) {
      return;
    }
    const normalized = snapshotLayout(layout, cols);
    if (normalized.length === 0) {
      return;
    }
    const savedMap = new Map(savedLayout.map((item) => [item.id, item]));
    let needsPersist = false;

    for (const item of normalized) {
      const saved = savedMap.get(item.id);
      if (!saved) {
        needsPersist = true;
        break;
      }
      if (saved.x !== item.x || saved.y !== item.y || saved.w !== item.w || saved.h !== item.h) {
        needsPersist = true;
        break;
      }
      if (item.widthPx != null && saved.widthPx !== item.widthPx) {
        needsPersist = true;
        break;
      }
      if (item.heightPx != null && saved.heightPx !== item.heightPx) {
        needsPersist = true;
        break;
      }
      const currentState = tileState[item.id] ?? tileStateRef.current[item.id];
      const savedLocked = Boolean(saved.locked);
      if (savedLocked !== Boolean(currentState?.locked)) {
        needsPersist = true;
        break;
      }
      const savedZoom = saved.zoom ?? null;
      const nextZoom = currentState?.locked ? MAX_UNLOCKED_ZOOM : currentState?.zoom ?? null;
      if (
        savedZoom !== null &&
        nextZoom !== null &&
        Math.abs(savedZoom - nextZoom) > 0.005 &&
        !currentState?.locked
      ) {
        needsPersist = true;
        break;
      }
    }

    if (!needsPersist) {
      for (const saved of savedLayout) {
        if (!layoutMap.has(saved.id)) {
          needsPersist = true;
          break;
        }
      }
    }

    if (!needsPersist) {
      return;
    }

    const signature = JSON.stringify(
      normalized.map((item) => ({
        id: item.id,
        x: item.x,
        y: item.y,
        w: item.w,
        h: item.h,
        widthPx: item.widthPx ?? null,
        heightPx: item.heightPx ?? null,
        zoom: item.zoom ?? null,
        locked: item.locked ?? null,
        toolbarPinned: item.toolbarPinned ?? null,
      })),
    );
    if (lastPersistSignatureRef.current === signature) {
      return;
    }
    lastPersistSignatureRef.current = signature;
    onLayoutPersist(normalized);
  }, [
    cols,
    isClient,
    layout,
    layoutMap,
    onLayoutPersist,
    savedLayout,
    snapshotLayout,
    tileState,
  ]);

  const gridContent = isClient ? (
    <div className="session-grid">
      <AutoGrid
        layout={layout}
        cols={cols}
        rowHeight={110}
        margin={[16, 16]}
        containerPadding={[8, 8]}
        compactType={null}
        preventCollision={false}
        draggableHandle=".session-tile-drag-grip"
        draggableCancel=".session-tile-actions"
        resizeHandle={renderResizeHandle}
        resizeHandles={['e', 's', 'se']}
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
          const view = tileState[session.session_id] ?? buildTileState();
          const resizeControl = resizeControls[session.session_id];

          return (
            <div key={session.session_id} className="session-grid-item" data-grid-session={session.session_id}>
              <TileCard
                session={session}
                role={role}
                isAgent={isAgent}
                assignments={agentAssignments}
                controllers={controllers}
                collapsed={collapsed}
                onToggleAssignments={() => toggleAssignments(session.session_id)}
                onOpenAssignment={onOpenAssignment}
                onSelect={() => onSelect(session)}
                onRemove={() => onRemove(session.session_id)}
                onToggleRole={() =>
                  onRequestRoleChange(session, role === 'agent' ? 'application' : 'agent')
                }
                onExpand={() => setExpanded(session)}
                onSnap={() => handleSnap(session.session_id)}
                onToggleLock={() => handleToggleLock(session.session_id)}
                onToolbarToggle={() => handleToolbarToggle(session.session_id)}
                resizeControl={resizeControl}
                managerUrl={managerUrl}
                viewerToken={viewerToken}
                view={view}
                onMeasure={(measurement) => handleMeasure(session.session_id, measurement)}
                onViewport={(dims) => handleViewportDimensions(session.session_id, dims)}
                onHostResizeStateChange={handleHostResizeStateChange}
              />
            </div>
          );
        })}
      </AutoGrid>
    </div>
  ) : (
    <div className="h-[520px] rounded-xl border border-border bg-card shadow-sm" />
  );

  useEffect(() => {
    if (!isClient) return;
    const wrapper = gridWrapperRef.current;
    if (!wrapper) return;
    const firstItem = wrapper.querySelector<HTMLElement>('.react-grid-item');
    if (firstItem) {
      const rect = firstItem.getBoundingClientRect();
      console.info(
        '[tile-layout] item-width',
        JSON.stringify({ width: rect.width, height: rect.height }),
      );
    } else {
      console.info('[tile-layout] item-width', 'missing react-grid-item');
    }
  }, [isClient, layout]);

  return (
    <div ref={gridWrapperRef} className="relative">
      {gridContent}
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
              fontSize={BASE_FONT_SIZE}
              locked={false}
              cropped={false}
              onHostResizeStateChange={handleHostResizeStateChange}
              onViewportDimensions={(dims) => handleViewportDimensions(expanded.session_id, dims)}
            />
          </div>
        </div>
      )}
    </div>
  );
}
