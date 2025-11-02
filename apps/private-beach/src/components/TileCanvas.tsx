import {
  forwardRef,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from 'react';
import dynamic from 'next/dynamic';
import type { Layout } from 'react-grid-layout';
import type { SessionSummary, BeachLayoutItem, SessionRole, ControllerPairing, CanvasLayout as ApiCanvasLayout } from '../lib/api';
import type { AssignmentEdge } from '../lib/assignments';
import { pairingStatusDisplay, formatCadenceLabel } from '../lib/pairings';
import { debugLog } from '../lib/debug';
import { SessionTerminalPreview } from './SessionTerminalPreview';
import type { HostResizeControlState } from './SessionTerminalPreviewClient';
import { Badge } from './ui/badge';
import { Button } from './ui/button';
import type { TerminalViewerState } from '../hooks/terminalViewerTypes';
import { emitTelemetry } from '../lib/telemetry';
import { extractSessionTitle } from '../lib/sessionMetadata';
import { extractTerminalStateDiff, type TerminalStateDiff } from '../lib/terminalHydrator';
import {
  sessionTileController,
  useTileSnapshot,
  useCanvasSnapshot,
  type TileMeasurementPayload,
} from '../controllers/sessionTileController';
import { useTileViewState, selectTileViewState } from '../controllers/gridSelectors';
import { extractGridLayoutSnapshot, gridSnapshotToReactGrid } from '../controllers/gridLayout';
import {
  applyGridAutosizeCommand,
  applyGridDragCommand,
  applyGridResizeCommand,
} from '../controllers/gridLayoutCommands';
import {
  defaultTileViewState,
  type PreviewMetrics,
  type TileMeasurements,
  type TileViewState,
} from '../controllers/gridViewState';

const AutoGrid = dynamic(() => import('./AutoGrid'), {
  ssr: false,
  loading: () => <div className="h-[520px] rounded-xl border border-border bg-card shadow-sm" />,
});

const DEFAULT_COLS = 128;
const DEFAULT_W = 32;
const DEFAULT_H = 28;
const MIN_W = 4;
const MIN_H = 4;
const ROW_HEIGHT = 12;
const GRID_MARGIN_X = 16;
const GRID_MARGIN_Y = 16;
const GRID_CONTAINER_PADDING_X = 8;
const GRID_CONTAINER_PADDING_Y = 8;
const UNLOCKED_MAX_W = 96;
const UNLOCKED_MAX_H = 96;
const TARGET_TILE_WIDTH = 448;
const MAX_UNLOCKED_ZOOM = 1;
const MIN_ZOOM = 0.05;
const DEFAULT_ZOOM = 1;
const DEFAULT_HOST_COLS = 80;
const DEFAULT_HOST_ROWS = 24;
const TERMINAL_PADDING_X = 48;
const TERMINAL_PADDING_Y = 56;
const BASE_FONT_SIZE = 14;
const BASE_CELL_WIDTH = 8;
const BASE_LINE_HEIGHT = Math.round(BASE_FONT_SIZE * 1.4);
const ZOOM_EPSILON = 0.02;
const UNLOCKED_MEASUREMENT_LIMIT = TARGET_TILE_WIDTH * 1.5;
const MAX_TILE_WIDTH_PX = 450;
const MAX_TILE_HEIGHT_PX = 450;
const GRID_LAYOUT_VERSION = 2;
const CROPPED_EPSILON = 0.02;
const MAX_NORMALIZE_ATTEMPTS = 3;
const LEGACY_GRID_COLS = 12;
const LEGACY_ROW_HEIGHT_PX = 110;
const LEGACY_MIN_W = 3;
const LEGACY_MIN_H = 3;

type TileViewStateMap = Record<string, TileViewState>;

const TILE_DEBUG_ENABLED = process.env.NEXT_PUBLIC_TILE_DEBUG === 'true';

function computeViewMeasurements(view: TileViewState): TileMeasurements | null {
  if (view.measurements) {
    return view.measurements;
  }
  if (view.preview) {
    const scale = view.locked ? MAX_UNLOCKED_ZOOM : view.zoom;
    return {
      width: view.preview.targetWidth * scale,
      height: view.preview.targetHeight * scale,
    };
  }
  return null;
}

function snapshotLayoutItems(
  layouts: Layout[],
  cols: number,
  viewStates: TileViewStateMap,
  tileOrder: string[],
): BeachLayoutItem[] {
  if (layouts.length === 0 || tileOrder.length === 0) {
    return [];
  }
  const allowed = new Set(tileOrder);
  const byId = new Map<string, BeachLayoutItem>();
  const effectiveCols = Math.max(DEFAULT_W, cols || DEFAULT_COLS);
  for (const item of layouts) {
    if (!allowed.has(item.i)) {
      continue;
    }
    const w = Math.min(effectiveCols, Math.max(MIN_W, Math.round(item.w)));
    const h = Math.max(MIN_H, Math.round(item.h));
    const x = Math.max(0, Math.min(Math.round(item.x), Math.max(0, effectiveCols - w)));
    const y = Math.max(0, Math.round(item.y));
    const view = viewStates[item.i] ?? defaultTileViewState();
    const measurement = computeViewMeasurements(view);
    const widthPx = measurement ? Math.round(measurement.width) : null;
    const heightPx = measurement ? Math.round(measurement.height) : null;
    const computedZoom = view.locked ? MAX_UNLOCKED_ZOOM : clampZoom(view.zoom, measurement);
    const normalizedZoom = Number.isFinite(computedZoom) ? Number(computedZoom.toFixed(3)) : undefined;
    const scaledW = Math.max(LEGACY_MIN_W, Math.round((w * LEGACY_GRID_COLS) / Math.max(1, effectiveCols)));
    const scaledH = Math.max(LEGACY_MIN_H, Math.round((h * LEGACY_ROW_HEIGHT_PX) / ROW_HEIGHT));
    const scaledX = Math.max(0, Math.round((x * LEGACY_GRID_COLS) / Math.max(1, effectiveCols)));
    const scaledY = Math.max(0, Math.round((y * LEGACY_ROW_HEIGHT_PX) / ROW_HEIGHT));
    const maxLegacyX = Math.max(0, LEGACY_GRID_COLS - scaledW);
    const clampedLegacyX = Math.max(0, Math.min(scaledX, maxLegacyX));
    const layoutItem: BeachLayoutItem = {
      id: item.i,
      x: clampedLegacyX,
      y: scaledY,
      w: scaledW,
      h: scaledH,
      gridCols: LEGACY_GRID_COLS,
      rowHeightPx: LEGACY_ROW_HEIGHT_PX,
      layoutVersion: GRID_LAYOUT_VERSION,
    };
    if (widthPx != null) {
      layoutItem.widthPx = widthPx;
    }
    if (heightPx != null) {
      layoutItem.heightPx = heightPx;
    }
    if (normalizedZoom != null) {
      layoutItem.zoom = normalizedZoom;
    }
    if (typeof view.locked === 'boolean') {
      layoutItem.locked = view.locked;
    }
    if (typeof view.toolbarPinned === 'boolean') {
      layoutItem.toolbarPinned = view.toolbarPinned;
    }
    byId.set(item.i, layoutItem);
  }
  return tileOrder
    .map((id) => byId.get(id))
    .filter((entry): entry is BeachLayoutItem => Boolean(entry));
}

function areLayoutsEquivalent(a: Layout[], b: Layout[]): boolean {
  if (a.length !== b.length) {
    return false;
  }
  const previous = new Map(a.map((item) => [item.i, item]));
  for (const next of b) {
    const match = previous.get(next.i);
    if (!match) {
      return false;
    }
    if (match.x !== next.x || match.y !== next.y || match.w !== next.w || match.h !== next.h) {
      return false;
    }
  }
  return true;
}

function serializeBeachLayoutItems(items: BeachLayoutItem[]): string {
  return items
    .slice()
    .sort((a, b) => a.id.localeCompare(b.id))
    .map((item) => {
      const width = item.widthPx != null ? Math.round(item.widthPx) : '';
      const height = item.heightPx != null ? Math.round(item.heightPx) : '';
      const zoom = item.zoom != null ? item.zoom.toFixed(3) : '';
      const locked = item.locked ? '1' : '0';
      const toolbarPinned = item.toolbarPinned ? '1' : '0';
      return [
        item.id,
        item.x,
        item.y,
        item.w,
        item.h,
        width,
        height,
        zoom,
        locked,
        toolbarPinned,
      ].join(':');
    })
    .join('|');
}

function tileDebugLog(...args: Parameters<typeof console.info>) {
  if (TILE_DEBUG_ENABLED) {
    console.info(...args);
  }
}

function tileDebugWarn(...args: Parameters<typeof console.warn>) {
  if (TILE_DEBUG_ENABLED) {
    console.warn(...args);
  }
}

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

export function clampZoom(value: number | undefined, measurement?: TileMeasurements | null): number {
  if (!Number.isFinite(value ?? Number.NaN)) {
    return DEFAULT_ZOOM;
  }
  let min = MIN_ZOOM;
  if (measurement) {
    const minWidthZoom = TARGET_TILE_WIDTH / measurement.width;
    min = Math.min(min, Math.max(MIN_ZOOM, minWidthZoom));
  }
  return Math.min(MAX_UNLOCKED_ZOOM, Math.max(min, Number(value)));
}

export function getColumnWidth(gridWidth: number | null, cols: number): number | null {
  if (gridWidth == null || gridWidth <= 0 || cols <= 0) {
    return null;
  }
  const availableWidth =
    gridWidth - GRID_MARGIN_X * Math.max(0, cols - 1) - GRID_CONTAINER_PADDING_X * 2;
  const baseWidth = availableWidth / cols;
  if (Number.isFinite(baseWidth) && baseWidth > 0) {
    return baseWidth;
  }
  const fallbackWidth = gridWidth / cols;
  return Number.isFinite(fallbackWidth) && fallbackWidth > 0 ? fallbackWidth : null;
}

export function estimateHostSize(cols: number | null, rows: number | null) {
  const c = cols && cols > 0 ? cols : DEFAULT_HOST_COLS;
  const r = rows && rows > 0 ? rows : DEFAULT_HOST_ROWS;
  const width = c * BASE_CELL_WIDTH + TERMINAL_PADDING_X;
  const height = r * BASE_LINE_HEIGHT + TERMINAL_PADDING_Y;
  return { width, height };
}

export function computeZoomForSize(
  measurements: TileMeasurements | null,
  hostCols: number | null,
  hostRows: number | null,
  viewportCols: number | null,
  viewportRows: number | null,
) {
  if (!measurements || measurements.width <= 0 || measurements.height <= 0) {
    return DEFAULT_ZOOM;
  }
  const resolvedHostCols =
    typeof hostCols === 'number' && hostCols > 0 ? hostCols : DEFAULT_HOST_COLS;
  const resolvedHostRows =
    typeof hostRows === 'number' && hostRows > 0 ? hostRows : DEFAULT_HOST_ROWS;
  const effectiveCols =
    typeof viewportCols === 'number' && viewportCols > 0
      ? Math.min(resolvedHostCols, viewportCols)
      : resolvedHostCols;
  const effectiveRows =
    typeof viewportRows === 'number' && viewportRows > 0
      ? Math.min(resolvedHostRows, viewportRows)
      : resolvedHostRows;
  const hostSize = estimateHostSize(effectiveCols, effectiveRows);
  const widthRatio = measurements.width / Math.max(1, hostSize.width);
  const heightRatio = measurements.height / Math.max(1, hostSize.height);
  const ratio = Math.min(widthRatio, heightRatio);
  return clampZoom(ratio);
}

function isSameMeasurement(a: TileMeasurements | null, b: TileMeasurements | null): boolean {
  if (!a && !b) return true;
  if (!a || !b) return false;
  return Math.abs(a.width - b.width) < 0.5 && Math.abs(a.height - b.height) < 0.5;
}

function isSamePreview(a: PreviewMetrics | null, b: PreviewMetrics | null): boolean {
  if (!a && !b) return true;
  if (!a || !b) return false;
  return (
    Math.abs(a.scale - b.scale) < 0.0001 &&
    Math.abs(a.targetWidth - b.targetWidth) < 0.5 &&
    Math.abs(a.targetHeight - b.targetHeight) < 0.5 &&
    Math.abs(a.rawWidth - b.rawWidth) < 0.5 &&
    Math.abs(a.rawHeight - b.rawHeight) < 0.5
  );
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

function isTileStateEqual(a: TileViewState, b: TileViewState): boolean {
  return (
    a.zoom === b.zoom &&
    a.locked === b.locked &&
    a.toolbarPinned === b.toolbarPinned &&
    a.hasHostDimensions === b.hasHostDimensions &&
    a.manualLayout === b.manualLayout &&
    a.layoutHostCols === b.layoutHostCols &&
    a.layoutHostRows === b.layoutHostRows &&
    a.hostCols === b.hostCols &&
    a.hostRows === b.hostRows &&
    a.viewportCols === b.viewportCols &&
    a.viewportRows === b.viewportRows &&
    a.previewStatus === b.previewStatus &&
    isSameMeasurement(a.measurements, b.measurements) &&
    isSamePreview(a.preview, b.preview) &&
    isSameLayoutDimensions(a.lastLayout, b.lastLayout)
  );
}

function diffTileViewState(prev: TileViewState, next: TileViewState): Partial<TileViewState> | null {
  const patch: Partial<TileViewState> = {};
  let changed = false;
  const assign = <K extends keyof TileViewState>(key: K, value: TileViewState[K]) => {
    patch[key] = value;
    changed = true;
  };

  if (prev.zoom !== next.zoom) assign('zoom', next.zoom);
  if (prev.locked !== next.locked) assign('locked', next.locked);
  if (prev.toolbarPinned !== next.toolbarPinned) assign('toolbarPinned', next.toolbarPinned);
  if (!isSameMeasurement(prev.measurements, next.measurements)) assign('measurements', next.measurements);
  if ((prev.hostCols ?? null) !== (next.hostCols ?? null)) assign('hostCols', next.hostCols);
  if ((prev.hostRows ?? null) !== (next.hostRows ?? null)) assign('hostRows', next.hostRows);
  if (prev.hasHostDimensions !== next.hasHostDimensions) assign('hasHostDimensions', next.hasHostDimensions);
  if ((prev.viewportCols ?? null) !== (next.viewportCols ?? null)) assign('viewportCols', next.viewportCols);
  if ((prev.viewportRows ?? null) !== (next.viewportRows ?? null)) assign('viewportRows', next.viewportRows);
  if (!isSameLayoutDimensions(prev.lastLayout, next.lastLayout)) assign('lastLayout', next.lastLayout);
  if (prev.layoutInitialized !== next.layoutInitialized) assign('layoutInitialized', next.layoutInitialized);
  if (prev.manualLayout !== next.manualLayout) assign('manualLayout', next.manualLayout);
  if ((prev.layoutHostCols ?? null) !== (next.layoutHostCols ?? null)) assign('layoutHostCols', next.layoutHostCols);
  if ((prev.layoutHostRows ?? null) !== (next.layoutHostRows ?? null)) assign('layoutHostRows', next.layoutHostRows);
  if (!isSamePreview(prev.preview, next.preview)) assign('preview', next.preview);
  if (prev.previewStatus !== next.previewStatus) assign('previewStatus', next.previewStatus);

  return changed ? patch : null;
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

function CopyIcon() {
  return (
    <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.2">
      <rect x="5.5" y="4.5" width="8" height="8" rx="1.6" />
      <path d="M4 11V5.5a2 2 0 0 1 2-2H11" strokeLinecap="round" />
    </svg>
  );
}

function CheckIcon() {
  return (
    <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.4">
      <path d="M4 8.5 7 11l5-6" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
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
  viewer: TerminalViewerState;
  view: TileViewState;
  onMeasure: (measurement: TileMeasurements) => void;
  onViewport: (
    sessionId: string,
    dims: {
      viewportRows: number;
      viewportCols: number;
      hostRows: number | null;
      hostCols: number | null;
    },
  ) => void;
  onPreviewStatusChange: (sessionId: string, status: 'connecting' | 'initializing' | 'ready' | 'error') => void;
  onPreviewMeasurementsChange: (
    sessionId: string,
    measurement: PreviewMetrics | null,
  ) => void;
  onHostResizeStateChange: (sessionId: string, state: HostResizeControlState | null) => void;
  isExpanded: boolean;
  cachedDiff?: TerminalStateDiff | null;
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
  viewer,
  view,
  onMeasure,
  onViewport,
  onPreviewStatusChange,
  onPreviewMeasurementsChange,
  onHostResizeStateChange,
  isExpanded,
  cachedDiff = null,
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
  const clampMeasurement = view.preview
    ? { width: view.preview.targetWidth, height: view.preview.targetHeight }
    : view.measurements;
  const zoomDisplay = view.locked ? MAX_UNLOCKED_ZOOM : clampZoom(view.zoom, clampMeasurement);
  const sessionName = useMemo(() => extractSessionTitle(session.metadata), [session.metadata]);
  const shortSessionId = useMemo(() => session.session_id.slice(0, 8), [session.session_id]);
  const dragGripClass = sessionName
    ? 'pointer-events-auto session-tile-drag-grip flex min-w-0 flex-col items-start gap-1 text-left text-muted-foreground'
    : 'pointer-events-auto session-tile-drag-grip flex items-center gap-2 text-[10px] uppercase tracking-[0.36em] text-muted-foreground';

  if (typeof window !== 'undefined') {
    try {
      tileDebugLog(
        '[tile-layout] tile-zoom',
        JSON.stringify({
          version: 'v1',
          sessionId: session.session_id,
          zoom: zoomDisplay,
          measurements: view.measurements,
          preview: view.preview,
          hostCols: view.hostCols,
          hostRows: view.hostRows,
          viewportCols: view.viewportCols,
          viewportRows: view.viewportRows,
        }),
      );
    } catch (err) {
      tileDebugLog('[tile-layout] tile-zoom', {
        version: 'v1',
        sessionId: session.session_id,
        zoom: zoomDisplay,
        measurements: view.measurements,
        preview: view.preview,
        hostCols: view.hostCols,
        hostRows: view.hostRows,
        viewportCols: view.viewportCols,
        viewportRows: view.viewportRows,
      });
    }
  }
  const fontSize = BASE_FONT_SIZE;
  const zoomLabel = `${Math.round(zoomDisplay * 100)}%`;
  const cropped =
    !view.locked &&
    view.measurements != null &&
    view.hostCols != null &&
    view.hostCols > 0 &&
    view.hostRows != null &&
    view.hostRows > 0 &&
    zoomDisplay < MAX_UNLOCKED_ZOOM - CROPPED_EPSILON;
  const resizeHint =
    resizeControl && resizeControl.canResize
      ? `Resize host to ${resizeControl.viewportCols}×${resizeControl.viewportRows}`
      : 'Host resize unavailable';

  const handlePreviewViewportDimensions = useCallback(
    (
      _sessionId: string,
      dims:
        | {
            viewportRows: number;
            viewportCols: number;
            hostRows: number | null;
            hostCols: number | null;
          }
        | undefined,
    ) => {
      if (!dims || typeof dims.viewportRows !== 'number' || typeof dims.viewportCols !== 'number') {
        return;
      }
      onViewport(session.session_id, dims);
    },
    [onViewport, session.session_id],
  );

  const handlePreviewMeasurements = useCallback(
    (
      sessionIdValue: string,
      measurement:
        | {
            scale: number;
            targetWidth: number;
            targetHeight: number;
            rawWidth: number;
            rawHeight: number;
            hostRows: number | null;
            hostCols: number | null;
            measurementVersion: number;
          }
        | null,
    ) => {
      if (sessionIdValue !== session.session_id) {
        return;
      }
      onPreviewMeasurementsChange(
        session.session_id,
        measurement
          ? {
              scale: measurement.scale,
              targetWidth: measurement.targetWidth,
              targetHeight: measurement.targetHeight,
              rawWidth: measurement.rawWidth,
              rawHeight: measurement.rawHeight,
              hostRows: measurement.hostRows,
              hostCols: measurement.hostCols,
              measurementVersion: measurement.measurementVersion,
            }
          : null,
      );
    },
    [onPreviewMeasurementsChange, session.session_id],
  );

  const handlePreviewStatusChange = useCallback(
    (status: 'connecting' | 'initializing' | 'ready' | 'error') => {
      onPreviewStatusChange(session.session_id, status);
    },
    [onPreviewStatusChange, session.session_id],
  );

  const [copyState, setCopyState] = useState<'idle' | 'copied'>('idle');

  const handleCopySessionId = useCallback(() => {
    if (typeof navigator === 'undefined' || !navigator.clipboard) {
      console.warn('[tile] clipboard API unavailable');
      return;
    }
    navigator.clipboard
      .writeText(session.session_id)
      .then(() => {
        setCopyState('copied');
      })
      .catch((error) => {
        console.error('[tile] failed to copy session id', {
          sessionId: session.session_id,
          error,
        });
      });
  }, [session.session_id]);

  useEffect(() => {
    if (copyState !== 'copied') {
      return;
    }
    const timeout = window.setTimeout(() => {
      setCopyState('idle');
    }, 1500);
    return () => {
      window.clearTimeout(timeout);
    };
  }, [copyState]);

  if (typeof window !== 'undefined') {
    tileDebugLog('[tile-layout] render-state', {
      sessionId: session.session_id,
      zoomDisplay,
      locked: view.locked,
      hasMeasurements: Boolean(view.measurements),
      measurement: view.measurements,
      hostRows: view.hostRows,
      hostCols: view.hostCols,
      viewportRows: view.viewportRows,
      viewportCols: view.viewportCols,
      cropped,
      layout: view.lastLayout,
    });
  }

  return (
    <div
      className={`group relative flex h-full flex-col overflow-hidden rounded-xl border bg-card text-card-foreground shadow-sm transition-shadow ${
        isAgent && assignments.length > 0 ? 'border-primary/60' : 'border-border'
      }`}
      data-session-id={session.session_id}
    >
      <div
        className={`pointer-events-none absolute inset-x-2 top-2 z-20 flex items-center justify-between rounded-full bg-background/80 px-3 py-1 text-[11px] font-medium text-muted-foreground shadow-sm backdrop-blur transition-opacity ${toolbarVisibleClass}`}
      >
        <button type="button" className={dragGripClass} onDoubleClick={onToolbarToggle}>
          {sessionName ? (
            <>
              <span className="max-w-[240px] truncate text-sm font-semibold text-foreground">
                {sessionName}
              </span>
              <div className="flex items-center gap-1 text-[10px] uppercase tracking-[0.28em] text-muted-foreground">
                <span className="rounded border border-border/60 bg-background/70 px-2 py-0.5 font-mono uppercase text-[10px] tracking-tight">
                  {shortSessionId}
                </span>
              </div>
            </>
          ) : (
            <span className="rounded border border-border/60 bg-background/70 px-2 py-0.5 font-mono uppercase text-[10px] tracking-tight">
              {shortSessionId}
            </span>
          )}
        </button>
        <div className="pointer-events-auto flex items-center gap-2">
          <IconButton
            title={copyState === 'copied' ? 'Copied session ID' : 'Copy session ID'}
            ariaLabel={copyState === 'copied' ? 'Session ID copied' : 'Copy session ID'}
            onClick={handleCopySessionId}
          >
            {copyState === 'copied' ? <CheckIcon /> : <CopyIcon />}
          </IconButton>
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
      <div className="space-y-3 pt-9">
        <div
          ref={contentRef}
          className="relative overflow-hidden rounded-lg border border-border/60 bg-neutral-900"
        >
          {!isExpanded ? (
            <SessionTerminalPreview
              sessionId={session.session_id}
              privateBeachId={session.private_beach_id}
              managerUrl={managerUrl}
              token={viewerToken}
              harnessType={session.harness_type}
              className="w-full"
              onHostResizeStateChange={onHostResizeStateChange}
          onViewportDimensions={handlePreviewViewportDimensions}
          onPreviewStatusChange={handlePreviewStatusChange}
          onPreviewMeasurementsChange={handlePreviewMeasurements}
          fontSize={fontSize}
          scale={zoomDisplay}
          locked={view.locked}
          cropped={cropped}
          viewer={viewer}
          cachedStateDiff={cachedDiff ?? undefined}
        />
          ) : (
            <div className="flex h-full w-full items-center justify-center bg-neutral-950/90 text-xs text-muted-foreground">
              <span>Expanded view active…</span>
            </div>
          )}
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

type SessionTileProps = {
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
  onMeasure: (measurement: TileMeasurements) => void;
  onViewport: (
    sessionId: string,
    dims: {
      viewportRows: number;
      viewportCols: number;
      hostRows: number | null;
      hostCols: number | null;
    },
  ) => void;
  onPreviewStatusChange: (sessionId: string, status: 'connecting' | 'initializing' | 'ready' | 'error') => void;
  onPreviewMeasurementsChange: (sessionId: string, measurement: PreviewMetrics | null) => void;
  onHostResizeStateChange: (sessionId: string, state: HostResizeControlState | null) => void;
  isExpanded: boolean;
  className?: string;
  style?: CSSProperties;
  viewerOverride?: TerminalViewerState | null;
  cachedDiff?: TerminalStateDiff | null;
};

const SessionTile = forwardRef<HTMLDivElement, SessionTileProps>(
  (
    {
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
      onMeasure,
      onViewport,
      onPreviewStatusChange,
      onPreviewMeasurementsChange,
      onHostResizeStateChange,
      isExpanded,
      className,
      style,
      viewerOverride,
      cachedDiff = null,
    },
    ref,
  ) => {
    const handleTilePreviewStatusChange = useCallback(
      (status: 'connecting' | 'initializing' | 'ready' | 'error') => {
        onPreviewStatusChange(session.session_id, status);
      },
      [onPreviewStatusChange, session.session_id],
    );

    const handleTilePreviewMeasurementsChange = useCallback(
      (sessionIdValue: string, measurement: PreviewMetrics | null) => {
        onPreviewMeasurementsChange(sessionIdValue, measurement);
        sessionTileController.enqueueMeasurement(
          sessionIdValue,
          measurement ? (measurement as TileMeasurementPayload) : null,
          'dom',
        );
      },
      [onPreviewMeasurementsChange],
    );

  const trimmedToken = viewerToken?.trim() ?? '';
  const tileSnapshot = useTileSnapshot(session.session_id);
  const view = useTileViewState(session.session_id);
  const effectiveViewer = viewerOverride ?? tileSnapshot.viewer;
  const transportVersion = (effectiveViewer as any).transportVersion ?? 0;

  const viewerSnapshot = useMemo<TerminalViewerState>(() => {
    return {
      store: effectiveViewer.store,
      transport: effectiveViewer.transport,
      transportVersion,
      connecting: effectiveViewer.connecting,
      error: effectiveViewer.error,
      status: effectiveViewer.status,
      secureSummary: effectiveViewer.secureSummary,
      latencyMs: effectiveViewer.latencyMs,
    };
  }, [
    effectiveViewer.store,
    effectiveViewer.transport,
    transportVersion,
    effectiveViewer.connecting,
    effectiveViewer.error,
    effectiveViewer.status,
    effectiveViewer.secureSummary,
    effectiveViewer.latencyMs,
  ]);
  const viewerConfigSummaryRef = useRef<string | null>(null);
  useEffect(() => {
    if (typeof window === 'undefined') return;
    const payload = {
      sessionId: session.session_id,
      usingController: viewerOverride == null,
      hasOverride: Boolean(viewerOverride),
      manualLayout: view.manualLayout,
      locked: view.locked,
    };
    const signature = JSON.stringify(payload);
    if (viewerConfigSummaryRef.current !== signature) {
      viewerConfigSummaryRef.current = signature;
      tileDebugLog('[tile-viewer] config', payload);
    }
  }, [session.session_id, viewerOverride, view.locked, view.manualLayout]);

  useEffect(() => {
    const transportType =
      viewerSnapshot.transport?.constructor?.name ??
      (viewerSnapshot.transport ? 'custom' : null);
    tileDebugLog('[tile-viewer] snapshot', {
      sessionId: session.session_id,
      status: viewerSnapshot.status,
      connecting: viewerSnapshot.connecting,
      latencyMs: viewerSnapshot.latencyMs,
      transportType,
      transportVersion: viewerSnapshot.transportVersion,
      hasStore: Boolean(viewerSnapshot.store),
    });
    return () => {
      tileDebugLog('[tile-viewer] snapshot-dispose', {
        sessionId: session.session_id,
      });
    };
  }, [
    session.session_id,
    viewerSnapshot,
  ]);

  const handlePreviewViewportDimensions = useCallback(
    (
      _sessionId: string,
      dims: {
        viewportRows: number;
        viewportCols: number;
        hostRows: number | null;
        hostCols: number | null;
      } | undefined,
    ) => {
      if (!dims || typeof dims.viewportRows !== 'number' || typeof dims.viewportCols !== 'number') {
        return;
      }
      onViewport(session.session_id, dims);
    },
    [onViewport, session.session_id],
  );

  const combinedClassName = className ? `session-grid-item ${className}` : 'session-grid-item';

  useEffect(() => {
    tileDebugLog('[tile-diag] session-tile mount', {
      sessionId: session.session_id,
      viewerTokenProvided: Boolean(viewerToken),
    });
    return () => {
      tileDebugLog('[tile-diag] session-tile unmount', {
        sessionId: session.session_id,
      });
    };
  }, [session.session_id, viewerToken]);

  return (
    <div
      ref={ref}
      className={combinedClassName}
      style={style}
      data-grid-session={session.session_id}
    >
      <TileCard
        session={session}
        role={role}
        isAgent={isAgent}
        assignments={assignments}
        controllers={controllers}
        collapsed={collapsed}
        onToggleAssignments={onToggleAssignments}
        onOpenAssignment={onOpenAssignment}
        onSelect={onSelect}
        onRemove={onRemove}
        onToggleRole={onToggleRole}
        onExpand={onExpand}
        onSnap={onSnap}
        onToggleLock={onToggleLock}
        onToolbarToggle={onToolbarToggle}
        resizeControl={resizeControl}
        managerUrl={managerUrl}
        viewerToken={viewerToken}
        viewer={viewerSnapshot}
        view={view}
        onMeasure={onMeasure}
        onViewport={handlePreviewViewportDimensions}
        onPreviewStatusChange={handleTilePreviewStatusChange}
        onPreviewMeasurementsChange={handleTilePreviewMeasurementsChange}
        onHostResizeStateChange={onHostResizeStateChange}
        isExpanded={isExpanded}
      />
    </div>
  );
  },
);
SessionTile.displayName = 'SessionTile';

function ExpandedSessionPreview({
  session,
  managerUrl,
  viewerToken,
  onHostResizeStateChange,
  onViewportDimensions,
  onPreviewMeasurementsChange,
  onPreviewStatusChange,
}: {
  session: SessionSummary;
  managerUrl: string;
  viewerToken: string | null;
  onHostResizeStateChange: (sessionId: string, state: HostResizeControlState | null) => void;
  onViewportDimensions: (
    sessionId: string,
    dims: {
      viewportRows: number;
      viewportCols: number;
      hostRows: number | null;
      hostCols: number | null;
    },
  ) => void;
  onPreviewMeasurementsChange: (sessionId: string, measurement: PreviewMetrics | null) => void;
  onPreviewStatusChange: (status: 'connecting' | 'initializing' | 'ready' | 'error') => void;
}) {
  const snapshot = useTileSnapshot(session.session_id);
  const cachedDiff = snapshot.cachedDiff ?? undefined;
  const isTestEnvironment = typeof process !== 'undefined' && process.env.NODE_ENV === 'test';

  if (isTestEnvironment) {
    return <div data-testid="expanded-preview-placeholder" className="h-full w-full bg-neutral-950/20" />;
  }

  return (
    <SessionTerminalPreview
      sessionId={session.session_id}
      privateBeachId={session.private_beach_id}
      managerUrl={managerUrl}
      token={viewerToken}
      variant="full"
      harnessType={session.harness_type}
      className="h-full w-full"
      fontSize={BASE_FONT_SIZE}
      locked={false}
      cropped={false}
      onHostResizeStateChange={(sessionIdArg, state) => {
        onHostResizeStateChange(sessionIdArg ?? session.session_id, state);
      }}
      onViewportDimensions={(sessionIdArg, dims) => {
        if (!dims) {
          return;
        }
        onViewportDimensions(sessionIdArg ?? session.session_id, dims);
      }}
      onPreviewMeasurementsChange={(sessionIdArg, measurement) => {
        onPreviewMeasurementsChange(
          sessionIdArg ?? session.session_id,
          measurement
            ? {
                scale: measurement.scale,
                targetWidth: measurement.targetWidth,
                targetHeight: measurement.targetHeight,
                rawWidth: measurement.rawWidth,
                rawHeight: measurement.rawHeight,
                hostRows: measurement.hostRows,
                hostCols: measurement.hostCols,
                measurementVersion: measurement.measurementVersion,
              }
            : null,
        );
      }}
      onPreviewStatusChange={onPreviewStatusChange}
      viewer={snapshot.viewer}
      cachedStateDiff={cachedDiff}
    />
  );
}

type Props = {
  tiles: SessionSummary[];
  onRemove: (sessionId: string) => void;
  onSelect: (s: SessionSummary) => void;
  viewerToken: string | null;
  managerToken?: string | null;
  managerUrl: string;
  preset?: 'grid2x2' | 'onePlusThree' | 'focus';
  savedLayout?: BeachLayoutItem[];
  onLayoutPersist?: (layout: BeachLayoutItem[]) => void;
  roles: Map<string, SessionRole>;
  assignmentsByAgent: Map<string, AssignmentEdge[]>;
  assignmentsByApplication: Map<string, ControllerPairing[]>;
  onRequestRoleChange: (session: SessionSummary, role: SessionRole) => void;
  onOpenAssignment: (pairing: ControllerPairing) => void;
  viewerOverrides?: Partial<Record<string, TerminalViewerState>>;
};

export default function TileCanvas({
  tiles,
  onRemove,
  onSelect,
  viewerToken,
  managerToken = null,
  managerUrl,
  preset = 'grid2x2',
  savedLayout,
  onLayoutPersist,
  roles,
  assignmentsByAgent,
  assignmentsByApplication,
  onRequestRoleChange,
  onOpenAssignment,
  viewerOverrides,
}: Props) {
  const [expanded, setExpanded] = useState<SessionSummary | null>(null);
  const [isClient, setIsClient] = useState(false);
  const [collapsedAssignments, setCollapsedAssignments] = useState<Record<string, boolean>>({});
  const [resizeControls, setResizeControls] = useState<Record<string, HostResizeControlState>>({});
  const [gridWidth, setGridWidth] = useState<number | null>(null);
  const [gridElementNode, setGridElementNode] = useState<HTMLElement | null>(null);

  const cachedTerminalDiffs = useMemo<Record<string, TerminalStateDiff>>(() => {
    const map: Record<string, TerminalStateDiff> = {};
    for (const session of tiles) {
      const diff = extractTerminalStateDiff(session.metadata);
      if (diff) {
        map[session.session_id] = diff;
        tileDebugLog('[terminal-hydrate][tile-canvas][metadata]', {
          sessionId: session.session_id,
          sequence: diff.sequence ?? null,
        });
      }
    }
    return map;
  }, [tiles]);

  const effectiveViewerOverrides = useMemo<Record<string, TerminalViewerState | null | undefined>>(() => {
    return viewerOverrides ?? {};
  }, [viewerOverrides]);

  const hydrateKeyRef = useRef<string | null>(null);
  const canvasSnapshot = useCanvasSnapshot();
  const gridSnapshot = useMemo(() => extractGridLayoutSnapshot(canvasSnapshot.layout), [canvasSnapshot]);
  const cols = gridSnapshot.gridCols ?? DEFAULT_COLS;
  const rowHeightPx = gridSnapshot.rowHeightPx ?? ROW_HEIGHT;
  const layoutVersion = gridSnapshot.layoutVersion ?? GRID_LAYOUT_VERSION;
  const viewStateMap = useMemo<TileViewStateMap>(() => {
    const result: TileViewStateMap = {};
    for (const [tileId, metadata] of Object.entries(gridSnapshot.tiles)) {
      result[tileId] = selectTileViewState(metadata);
    }
    return result;
  }, [gridSnapshot]);

  const tileSignature = useMemo(() => {
    return tiles
      .map((session) => `${session.session_id}:${session.private_beach_id ?? ''}`)
      .sort()
      .join('|');
  }, [tiles]);

  const savedLayoutSignature = useMemo(() => {
    if (!savedLayout || savedLayout.length === 0) {
      return 'empty';
    }
    return savedLayout
      .map((item) => `${item.id}:${item.x}:${item.y}:${item.w}:${item.h}`)
      .sort()
      .join('|');
  }, [savedLayout]);

  const savedLayoutPersistSignature = useMemo(() => {
    if (!savedLayout || savedLayout.length === 0) {
      return 'empty';
    }
    return serializeBeachLayoutItems(savedLayout);
  }, [savedLayout]);

  const viewerOverrideSignature = useMemo(() => {
    return Object.entries(effectiveViewerOverrides)
      .map(([id, state]) => `${id}:${state?.status ?? 'null'}:${state?.connecting ? '1' : '0'}`)
      .sort()
      .join('|');
  }, [effectiveViewerOverrides]);

  const hydrateKey = useMemo(() => {
    return [tileSignature, savedLayoutSignature, managerUrl, managerToken ?? '', viewerToken ?? '', viewerOverrideSignature].join('::');
  }, [managerToken, managerUrl, savedLayoutSignature, tileSignature, viewerOverrideSignature, viewerToken]);

  const tileOrder = useMemo(() => tiles.map((t) => t.session_id), [tiles]);

  const exportLegacyGridItems = useCallback((): BeachLayoutItem[] => {
    const gridSnapshot = sessionTileController.getGridLayoutSnapshot();
    const entries: Layout[] = [];
    for (const id of tileOrder) {
      const metadata = gridSnapshot.tiles[id];
      if (!metadata) {
        continue;
      }
      const layoutUnits = metadata.layout;
      const clampedW = Math.min(Math.max(layoutUnits.w, MIN_W), cols);
      const maxX = Math.max(0, cols - clampedW);
      const x = Math.min(Math.max(layoutUnits.x, 0), maxX);
      const y = Math.max(layoutUnits.y, 0);
      const h = Math.max(layoutUnits.h, MIN_H);
      entries.push({
        i: id,
        x,
        y,
        w: clampedW,
        h,
        minW: MIN_W,
        minH: MIN_H,
      });
    }
    if (entries.length === 0) {
      return [];
    }
    console.log('[exportLegacy] entries', entries);
    const snapshotMap = new Map<string, ReturnType<typeof sessionTileController.getTileSnapshot>>();
    return snapshotLayoutItems(entries, cols, viewStateMap, tileOrder).map((item) => {
      const snapshot = snapshotMap.get(item.id) ?? sessionTileController.getTileSnapshot(item.id);
      snapshotMap.set(item.id, snapshot);
      const viewState = selectTileViewState(snapshot.grid);
      const measurement = computeViewMeasurements(viewState);
      if (measurement) {
        const widthUnitsDefault = (measurement.width / TARGET_TILE_WIDTH) * DEFAULT_W;
        const scaledWidth = Math.round((widthUnitsDefault * LEGACY_GRID_COLS) / DEFAULT_COLS);
        item.w = Math.max(LEGACY_MIN_W, Math.min(LEGACY_GRID_COLS, scaledWidth));
      }
      return item;
    });
  }, [cols, tileOrder, viewStateMap]);

  const lastPersistSignatureRef = useRef<string | null>(null);
  const pendingPersistSignatureRef = useRef<string | null>(null);
  const normalizedPersistRef = useRef(false);
  const handlePersistLayout = useCallback(
    (_layout: ApiCanvasLayout) => {
      if (!onLayoutPersist) {
        return;
      }
      const legacyItems = exportLegacyGridItems();
      console.log('[persist] legacy items length', legacyItems.length);
      if (legacyItems.length === 0) {
        return;
      }
      const signature = serializeBeachLayoutItems(legacyItems);
      pendingPersistSignatureRef.current = null;
      if (lastPersistSignatureRef.current === signature) {
        return;
      }
      lastPersistSignatureRef.current = signature;
      onLayoutPersist(legacyItems);
    },
    [exportLegacyGridItems, onLayoutPersist],
  );

  const previousViewStateRef = useRef<TileViewStateMap>({});
  const gridWrapperRef = useRef<HTMLDivElement | null>(null);
  const controllerHydratedRef = useRef(false);

  const clampLayoutItems = useCallback(
    (layouts: Layout[], colsValue: number): Layout[] => {
      const effectiveCols = Math.max(DEFAULT_W, colsValue || DEFAULT_COLS);
      return layouts.map((item) => {
        const state = viewStateMap[item.i];
        const restrict = !state?.lastLayout;
        const { w, h } = clampGridSize(item.w, item.h, state, effectiveCols, restrict);
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
    [viewStateMap],
  );

  useEffect(() => {
    for (const [sessionId, diff] of Object.entries(cachedTerminalDiffs)) {
      sessionTileController.setCachedDiff(sessionId, diff ?? null);
    }
  }, [cachedTerminalDiffs]);

  useEffect(() => {
    lastPersistSignatureRef.current = savedLayoutPersistSignature;
    normalizedPersistRef.current = false;
    pendingPersistSignatureRef.current = null;
  }, [savedLayoutPersistSignature]);



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
      tileDebugLog('[tile-diag] apply-width', { width, fallback, targetWidth });
      setGridWidth(targetWidth);
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
  }, [isClient]);

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

  const layout = useMemo(() => {
    const derived = gridSnapshotToReactGrid(canvasSnapshot.layout, {
      fallbackCols: cols,
      minW: MIN_W,
      minH: MIN_H,
    }).map((item) => ({ ...item, maxW: cols }));
    tileDebugLog(
      '[tile-layout] ensure',
      JSON.stringify({
        cols,
        items: derived.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })),
      }),
    );
    debugLog('tile-layout', 'ensure layout', {
      cols,
      layout: derived.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })),
    });
    return derived;
  }, [canvasSnapshot, cols]);
  const layoutSignature = useMemo(() => {
    return JSON.stringify(layout.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })));
  }, [layout]);

  useEffect(() => {
    if (TILE_DEBUG_ENABLED) {
      tileDebugLog('[tile-layout] layout-signature', layoutSignature);
    }
  }, [layoutSignature]);

  const layoutForRender = useMemo(
    () =>
      layout.map((item) => ({
        ...item,
        data: {
          x: item.x,
          y: item.y,
          w: item.w,
          h: item.h,
          minW: item.minW,
          maxW: item.maxW,
          minH: item.minH,
          maxH: item.maxH,
        },
      })),
    [layout],
  );

  const layoutMap = useMemo(() => {
    const map = new Map<string, Layout>();
    layout.forEach((item) => map.set(item.i, item));
    return map;
  }, [layout]);

  const gridCommandContext = useMemo(
    () => ({
      cols,
      rowHeightPx,
      layoutVersion,
    }),
    [cols, layoutVersion, rowHeightPx],
  );

  const applyReactGridMutation = useCallback(
    (kind: 'drag' | 'resize' | 'autosize', nextLayouts: Layout[], reason: string, persist: boolean) => {
      if (nextLayouts.length === 0) {
        return;
      }
      const layoutsCopy = nextLayouts.map((item) => ({ ...item }));
      if (persist) {
      }
      sessionTileController.applyGridCommand(
        reason,
        (layout) => {
          switch (kind) {
            case 'drag':
              return applyGridDragCommand(layout, layoutsCopy, gridCommandContext);
            case 'autosize':
              return applyGridAutosizeCommand(layout, layoutsCopy, gridCommandContext);
            case 'resize':
            default:
              return applyGridResizeCommand(layout, layoutsCopy, gridCommandContext);
          }
        },
        { suppressPersist: !persist },
      );
    },
    [gridCommandContext],
  );

  const normalizeSavedLayout = useCallback(() => {
    if (!controllerHydratedRef.current || !isClient || layout.length === 0) {
      return;
    }
    const normalized = clampLayoutItems(layout, cols);
    if (normalized.length === 0) {
      return;
    }
    if (!areLayoutsEquivalent(layout, normalized)) {
      applyReactGridMutation('autosize', normalized, 'grid-normalize', true);
      return;
    }
    const exportedLegacy = exportLegacyGridItems();
    const exportedSignature = exportedLegacy.length === 0 ? 'empty' : serializeBeachLayoutItems(exportedLegacy);
    if (!savedLayout || savedLayout.length === 0) {
      if (
        exportedSignature !== 'empty' &&
        pendingPersistSignatureRef.current !== exportedSignature &&
        lastPersistSignatureRef.current !== exportedSignature
      ) {
        pendingPersistSignatureRef.current = exportedSignature;
        sessionTileController.requestPersist();
        if (!normalizedPersistRef.current) {
          normalizedPersistRef.current = true;
          handlePersistLayout(sessionTileController.getSnapshot().layout as ApiCanvasLayout);
        }
      }
      return;
    }
    const savedSignature = serializeBeachLayoutItems(savedLayout);
    if (savedSignature !== exportedSignature) {
      if (lastPersistSignatureRef.current === exportedSignature) {
        return;
      }
      if (pendingPersistSignatureRef.current === exportedSignature) {
        return;
      }
      pendingPersistSignatureRef.current = exportedSignature;
      sessionTileController.requestPersist();
      if (!normalizedPersistRef.current) {
        normalizedPersistRef.current = true;
        handlePersistLayout(sessionTileController.getSnapshot().layout as ApiCanvasLayout);
      }
    }
  }, [applyReactGridMutation, clampLayoutItems, cols, exportLegacyGridItems, handlePersistLayout, isClient, layout, savedLayout]);

  useLayoutEffect(() => {
    if (hydrateKeyRef.current === hydrateKey) {
      controllerHydratedRef.current = true;
      return;
    }
    hydrateKeyRef.current = hydrateKey;
    controllerHydratedRef.current = false;
    sessionTileController.hydrate({
      layout: null,
      gridLayoutItems: savedLayout ?? [],
      sessions: tiles,
      agents: [],
      privateBeachId: null,
      managerUrl,
      managerToken,
      viewerToken,
      viewerStateOverrides: effectiveViewerOverrides,
      onPersistLayout: onLayoutPersist ? handlePersistLayout : undefined,
    });
    controllerHydratedRef.current = true;
    normalizeSavedLayout();
  }, [
    effectiveViewerOverrides,
    handlePersistLayout,
    hydrateKey,
    managerToken,
    managerUrl,
    normalizeSavedLayout,
    onLayoutPersist,
    savedLayout,
    savedLayoutSignature,
    tiles,
    viewerToken,
  ]);

  useLayoutEffect(() => {
    normalizeSavedLayout();
  }, [normalizeSavedLayout]);


  useEffect(() => {
    if (!isClient || !gridWidth || gridWidth <= 0 || cols <= 0) {
      return;
    }
    const columnWidth = getColumnWidth(gridWidth, cols);
    if (columnWidth == null) {
      return;
    }
    tileDebugLog('[tile-diag] autosize-start', {
      cols,
      gridWidth,
      columnWidth,
      tileCount: layout.length,
    });
    const adjustments: Array<{
      id: string;
      w: number;
      h: number;
      hostCols: number;
      hostRows: number;
      widthPx: number;
      heightPx: number;
    }> = [];
    const initializedTiles: Array<{
      id: string;
      hostCols: number;
      hostRows: number;
      widthPx: number;
      heightPx: number;
    }> = [];
    layout.forEach((item) => {
      const state = viewStateMap[item.i];
      const hasPreview = Boolean(state?.preview);
      const hostCols = (() => {
        if (state?.hostCols && state.hostCols > 0) return state.hostCols;
        return DEFAULT_HOST_COLS;
      })();
      const hostRows = (() => {
        if (state?.hostRows && state.hostRows > 0) return state.hostRows;
        return DEFAULT_HOST_ROWS;
      })();
      if (!state || state.locked || state.manualLayout) {
        return;
      }
      if (!hasPreview && (!state.hasHostDimensions || hostCols <= 0 || hostRows <= 0)) {
        return;
      }
      const sizedForHost =
        !hasPreview &&
        state.layoutInitialized &&
        state.layoutHostCols === hostCols &&
        state.layoutHostRows === hostRows;
      if (sizedForHost) {
        return;
      }
      const measurement = state.preview && state.measurements ? state.measurements : null;
      let targetWidthPx: number;
      let targetHeightPx: number;
      let effectiveCols: number | null = null;
      let effectiveRows: number | null = null;
      let hostScale: number | null = null;
      if (measurement) {
        targetWidthPx = Math.max(
          MIN_W * columnWidth,
          Math.min(MAX_TILE_WIDTH_PX, Math.round(measurement.width)),
        );
      targetHeightPx = Math.max(
          MIN_H * rowHeightPx,
          Math.min(MAX_TILE_HEIGHT_PX, Math.round(measurement.height)),
        );
      } else {
        effectiveCols = (() => {
          const viewport = state?.viewportCols && state.viewportCols > 0 ? state.viewportCols : null;
          const host = hostCols && hostCols > 0 ? hostCols : null;
          if (viewport != null && host != null) {
            return Math.min(host, viewport);
          }
          return viewport ?? host;
        })();
        effectiveRows = (() => {
          const viewport = state?.viewportRows && state.viewportRows > 0 ? state.viewportRows : null;
          const host = hostRows && hostRows > 0 ? hostRows : null;
          if (viewport != null && host != null) {
            return Math.min(host, viewport);
          }
          return viewport ?? host;
        })();
        const hostSize = estimateHostSize(effectiveCols, effectiveRows);
        hostScale = Math.min(
          1,
          MAX_TILE_WIDTH_PX / hostSize.width,
          MAX_TILE_HEIGHT_PX / hostSize.height,
        );
        targetWidthPx = Math.max(
          MIN_W * columnWidth,
          Math.min(MAX_TILE_WIDTH_PX, hostSize.width * hostScale),
        );
        targetHeightPx = Math.max(
          MIN_H * rowHeightPx,
          Math.min(MAX_TILE_HEIGHT_PX, hostSize.height * hostScale),
        );
      }
      const computeWidthUnits = (widthPx: number) => {
        if (!Number.isFinite(widthPx) || widthPx <= 0) {
          return MIN_W;
        }
        const raw = widthPx / Math.max(columnWidth, 1e-6);
        return Math.max(MIN_W, Math.min(cols, Math.ceil(raw)));
      };
      const computeHeightUnits = (heightPx: number) => {
        if (!Number.isFinite(heightPx) || heightPx <= 0) {
          return MIN_H;
        }
        const raw = heightPx / Math.max(rowHeightPx, 1e-6);
        return Math.max(MIN_H, Math.ceil(raw));
      };
      const targetW = computeWidthUnits(targetWidthPx);
      const targetH = computeHeightUnits(targetHeightPx);
      const normalizedWidthPx = targetW * columnWidth;
      const normalizedHeightPx = targetH * rowHeightPx;
      if (targetW !== item.w || targetH !== item.h) {
        adjustments.push({
          id: item.i,
          w: targetW,
          h: targetH,
          hostCols,
          hostRows,
          widthPx: Math.round(normalizedWidthPx),
          heightPx: Math.round(normalizedHeightPx),
        });
      } else {
        initializedTiles.push({
          id: item.i,
          hostCols,
          hostRows,
          widthPx: Math.round(normalizedWidthPx),
          heightPx: Math.round(normalizedHeightPx),
        });
      }
      tileDebugLog('[tile-diag] autosize-eval-detail', {
        id: item.i,
        hostCols,
        hostRows,
        effectiveCols,
        effectiveRows,
        targetWidthPx,
        targetHeightPx,
        targetW,
        targetH,
        columnWidth,
        scale: hostScale,
        measurementSource: measurement ? 'preview' : 'host',
        previewMeasurement: measurement,
      });
    });
    tileDebugLog('[tile-diag] autosize-evaluated', {
      adjustments,
      initializedTiles,
    });
    if (adjustments.length === 0) {
      if (initializedTiles.length > 0) {
        initializedTiles.forEach(({ id, hostCols, hostRows, widthPx, heightPx }) => {
          const current = viewStateMap[id] ?? defaultTileViewState();
          if (
            !current.layoutInitialized ||
            current.layoutHostCols !== hostCols ||
            current.layoutHostRows !== hostRows ||
            current.manualLayout ||
            !current.hasHostDimensions ||
            !current.measurements ||
            !isSameMeasurement(current.measurements, { width: widthPx, height: heightPx })
          ) {
            const autoZoom = clampZoom(
              computeZoomForSize(
                { width: widthPx, height: heightPx },
                hostCols,
                hostRows,
                current.viewportCols ?? null,
                current.viewportRows ?? null,
              ),
            );
            const hasPreview = Boolean(current.preview);
            const nextState: TileViewState = {
              ...current,
              layoutInitialized: true,
              manualLayout: false,
              layoutHostCols: hostCols,
              layoutHostRows: hostRows,
              hasHostDimensions: true,
              measurements: hasPreview ? current.measurements : { width: widthPx, height: heightPx },
              zoom: current.locked ? MAX_UNLOCKED_ZOOM : hasPreview ? current.zoom : autoZoom,
            };
            const patch = diffTileViewState(current, nextState);
            if (patch) {
              sessionTileController.updateTileViewState(id, 'view-state.autosize.init', patch);
            }
            tileDebugLog('[tile-diag] autosize-initialize', {
              id,
              hostCols,
              hostRows,
              widthPx,
              heightPx,
              zoom: nextState.zoom,
              reason: 'alreadySized',
            });
          }
        });
      }
      return;
    }
    const adjustedLayouts = layout.map((item) => {
      const match = adjustments.find(({ id }) => id === item.i);
      if (!match) {
        return item;
      }
      const normalized = clampGridSize(match.w, match.h, viewStateMap[item.i], cols, false);
      return {
        ...item,
        w: normalized.w,
        h: normalized.h,
      };
    });
    const normalizedLayouts = clampLayoutItems(adjustedLayouts, cols);
    if (areLayoutsEquivalent(layout, normalizedLayouts)) {
      return;
    }
    applyReactGridMutation('autosize', normalizedLayouts, 'grid-autosize', false);
    adjustments.forEach(({ id, w, h, hostCols, hostRows, widthPx, heightPx }) => {
      const current = viewStateMap[id] ?? defaultTileViewState();
      const normalized = clampGridSize(w, h, current, cols, false);
      const autoZoom = clampZoom(
        computeZoomForSize(
          { width: widthPx, height: heightPx },
          hostCols,
          hostRows,
          current.viewportCols ?? null,
          current.viewportRows ?? null,
        ),
      );
      const hasPreview = Boolean(current.preview);
      const nextState: TileViewState = {
        ...current,
        lastLayout: { w: normalized.w, h: normalized.h },
        layoutInitialized: true,
        manualLayout: false,
        layoutHostCols: hostCols,
        layoutHostRows: hostRows,
        hasHostDimensions: true,
        measurements: hasPreview ? current.measurements : { width: widthPx, height: heightPx },
        zoom: current.locked ? MAX_UNLOCKED_ZOOM : hasPreview ? current.zoom : autoZoom,
      };
      const patch = diffTileViewState(current, nextState);
      if (patch) {
        sessionTileController.updateTileViewState(id, 'view-state.autosize.apply', patch);
      }
      tileDebugLog('[tile-diag] autosize-apply', {
        id,
        hostCols,
        hostRows,
        widthPx,
        heightPx,
        gridWidth,
        cols,
        gridUnits: { w: normalized.w, h: normalized.h },
        zoom: nextState.zoom,
      });
    });
  }, [applyReactGridMutation, clampLayoutItems, cols, gridWidth, isClient, layout, rowHeightPx, viewStateMap])


  const handleLayoutCommit = useCallback(
    (nextLayouts: Layout[], reason: 'drag-stop' | 'resize-stop' | 'state-change') => {
      const normalized = clampLayoutItems(nextLayouts, cols);
      tileDebugLog(
        '[tile-layout] commit',
        JSON.stringify({
          reason,
          cols,
          items: normalized.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })),
        }),
      );
          
    debugLog('tile-layout', 'layout commit', {
        reason,
        tileCount: normalized.length,
        layout: normalized.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })),
      });
      const { kind, controllerReason } =
        reason === 'drag-stop'
          ? { kind: 'drag' as const, controllerReason: 'grid-drag' }
          : reason === 'resize-stop'
            ? { kind: 'resize' as const, controllerReason: 'grid-resize' }
            : { kind: 'autosize' as const, controllerReason: 'grid-state' };
      applyReactGridMutation(kind, normalized, controllerReason, true);
      try {
        emitTelemetry('canvas.layout.persist', { reason, tiles: normalized.length });
      } catch {}
    },
    [applyReactGridMutation, clampLayoutItems, cols],
  );

  const handleLayoutChange = useCallback(
    (nextLayouts: Layout[]) => {
      tileDebugLog(
        '[tile-layout] onLayoutChange',
        JSON.stringify(nextLayouts.map(({ i, x, y, w, h }) => ({ i, x, y, w, h }))),
      );
      const normalized = clampLayoutItems(nextLayouts, cols);
      if (areLayoutsEquivalent(layout, normalized)) {
        tileDebugLog(
          '[tile-layout] onLayoutChange skip-equal',
          JSON.stringify(normalized.map(({ i, x, y, w, h }) => ({ i, x, y, w, h }))),
        );
        return;
      }
      tileDebugLog(
        '[tile-layout] onLayoutChange normalized',
        JSON.stringify(normalized.map(({ i, x, y, w, h }) => ({ i, x, y, w, h }))),
      );
          
    debugLog('tile-layout', 'layout change', {
        tileCount: normalized.length,
        layout: normalized.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })),
      });
      normalized.forEach((item) => {
        const current = viewStateMap[item.i] ?? defaultTileViewState();
        const dims = clampGridSize(item.w, item.h, current, cols, true);
        if (isSameLayoutDimensions(current.lastLayout, dims)) {
          return;
        }
        const nextState: TileViewState = current.layoutInitialized
          ? {
              ...current,
              lastLayout: dims,
              manualLayout: true,
              layoutHostCols: current.hostCols ?? current.layoutHostCols,
              layoutHostRows: current.hostRows ?? current.layoutHostRows,
            }
          : {
              ...current,
              lastLayout: dims,
            };
        const patch = diffTileViewState(current, nextState);
        if (patch) {
          const reason = current.layoutInitialized ? 'view-state.layout.manual' : 'view-state.layout.prime';
          sessionTileController.updateTileViewState(item.i, reason, patch);
        }
      });
      applyReactGridMutation('resize', normalized, 'grid-change', false);
    },
    [applyReactGridMutation, clampLayoutItems, cols, layout, viewStateMap],
  );

  const scheduleHostResize = useCallback((sessionId: string) => {
    if (typeof window === 'undefined') {
      return;
    }
    const control = resizeControls[sessionId];
    if (!control?.canResize) return;
    const state = viewStateMap[sessionId];
    const measurement = state?.measurements;
    const computeTarget = () => {
      const widthPx = Math.max(1, Math.round(measurement?.width ?? 0));
      const heightPx = Math.max(1, Math.round(measurement?.height ?? 0));
      // Derive rows/cols from visible tile size
      const innerWidth = Math.max(1, widthPx - TERMINAL_PADDING_X);
      const innerHeight = Math.max(1, heightPx - TERMINAL_PADDING_Y);
      const cols = Math.max(2, Math.floor(innerWidth / BASE_CELL_WIDTH));
      const rows = Math.max(2, Math.floor(innerHeight / BASE_LINE_HEIGHT));
      return { rows, cols };
    };
    const request = () => {
      const { rows, cols } = computeTarget();
      if (typeof window !== 'undefined') {
        try {
          console.info('[terminal][resize] request', {
            sessionId,
            rows,
            cols,
            reason: 'tile_locked_or_snap',
          });
        } catch {
          // ignore
        }
      }
    if (control.request) {
        control.request({ rows, cols });
      } else {
        control.trigger?.();
      }
    };
    // debounce to avoid bursts
    window.clearTimeout((scheduleHostResize as any)._t?.[sessionId]);
    (scheduleHostResize as any)._t = (scheduleHostResize as any)._t || {};
    (scheduleHostResize as any)._t[sessionId] = window.setTimeout(request, 180);
  }, [resizeControls, viewStateMap]);

  useEffect(() => {
    const previous = previousViewStateRef.current;
    Object.entries(viewStateMap).forEach(([id, state]) => {
      const before = previous[id];
      if (state.locked && (!before || !before.locked)) {
        scheduleHostResize(id);
      }
    });
    previousViewStateRef.current = viewStateMap;
  }, [viewStateMap, scheduleHostResize]);

  const handleTilePreviewMeasurementsChange = useCallback((sessionId: string, measurement: PreviewMetrics | null) => {
    const snapshot = sessionTileController.getTileSnapshot(sessionId);
    const currentState = selectTileViewState(snapshot.grid);
    if (!measurement) {
      sessionTileController.updateTileViewState(
        sessionId,
        'view-state.preview.measurement',
        {
          preview: null,
          measurements: currentState.manualLayout ? currentState.measurements : null,
        },
        { persist: false },
      );
      return;
    }
    const prevVersion = currentState.preview?.measurementVersion ?? 0;
    if (measurement.measurementVersion < prevVersion) {
      tileDebugLog('[tile-layout] preview-skip-stale', {
        sessionId,
        prevVersion,
        nextVersion: measurement.measurementVersion,
      });
      return;
    }
    const zoomFactor = currentState.locked ? MAX_UNLOCKED_ZOOM : currentState.zoom;
    sessionTileController.updateTileViewState(
      sessionId,
      'view-state.preview.measurement',
      {
        preview: measurement,
        measurements: {
          width: measurement.targetWidth * zoomFactor,
          height: measurement.targetHeight * zoomFactor,
        },
        hasHostDimensions: true,
      },
      { persist: false },
    );
  }, []);

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

  const dragStartTimeRef = useRef<Map<string, number>>(new Map());

  const handleDragStart = useCallback(
    (
      _next: Layout[],
      _oldItem: Layout,
      newItem: Layout,
      _placeholder: Layout,
      _event: MouseEvent,
      _element?: HTMLElement,
    ) => {
      try {
        dragStartTimeRef.current.set(newItem.i, typeof performance !== 'undefined' ? performance.now() : Date.now());
        emitTelemetry('canvas.drag.start', { id: newItem.i, x: newItem.x, y: newItem.y });
      } catch {}
    },
    [],
  );

  const handleDragStop = useCallback(
    (next: Layout[], _oldItem: Layout, newItem: Layout) => {
      try {
        const start = dragStartTimeRef.current.get(newItem.i) ?? null;
        const now = typeof performance !== 'undefined' ? performance.now() : Date.now();
        const latency = start != null ? Math.max(0, now - start) : undefined;
        emitTelemetry('canvas.drag.stop', { id: newItem.i, x: newItem.x, y: newItem.y, latency });
      } catch {}
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
      const state = viewStateMap[newItem.i] ?? defaultTileViewState();
      const effectiveCols = (() => {
        const viewport = state.viewportCols && state.viewportCols > 0 ? state.viewportCols : null;
        const host = state.hostCols && state.hostCols > 0 ? state.hostCols : null;
        if (viewport != null && host != null) {
          return Math.min(host, viewport);
        }
        return viewport ?? host;
      })();
      const effectiveRows = (() => {
        const viewport = state.viewportRows && state.viewportRows > 0 ? state.viewportRows : null;
        const host = state.hostRows && state.hostRows > 0 ? state.hostRows : null;
        if (viewport != null && host != null) {
          return Math.min(host, viewport);
        }
        return viewport ?? host;
      })();
      const hostSize = estimateHostSize(effectiveCols, effectiveRows);
      const boundedHostWidth = Math.min(hostSize.width, MAX_TILE_WIDTH_PX);
      const boundedHostHeight = Math.min(hostSize.height, MAX_TILE_HEIGHT_PX);
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
          const zoomCandidate = computeZoomForSize(
            { width: widthPx, height: heightPx },
            state.hostCols,
            state.hostRows,
            state.viewportCols ?? null,
            state.viewportRows ?? null,
          );
          if (zoomCandidate >= MAX_UNLOCKED_ZOOM - ZOOM_EPSILON) {
            const unitWidthPx = newItem.w > 0 ? widthPx / newItem.w : widthPx;
            const unitHeightPx = newItem.h > 0 ? heightPx / newItem.h : heightPx;
            if (unitWidthPx > 0 && unitHeightPx > 0) {
              const targetWUnits = Math.max(MIN_W, Math.round(boundedHostWidth / unitWidthPx));
              const targetHUnits = Math.max(MIN_H, Math.round(boundedHostHeight / unitHeightPx));
              adjustedLayouts = nextLayouts.map((item) =>
                item.i === newItem.i ? { ...item, w: targetWUnits, h: targetHUnits } : item,
              );
            }
          }
        }
      }

      const normalized = clampLayoutItems(adjustedLayouts, cols);
      try {
        emitTelemetry('canvas.resize.stop', {
          id: newItem.i,
          w: newItem.w,
          h: newItem.h,
          widthPx,
          heightPx,
          hostWidth: boundedHostWidth,
          hostHeight: boundedHostHeight,
        });
      } catch {}
      handleLayoutChange(normalized);
      handleLayoutCommit(normalized, 'resize-stop');
      const layoutEntry = normalized.find((item) => item.i === newItem.i);
      if (layoutEntry) {
        const columnWidth = getColumnWidth(gridWidth, cols);
        const fallbackColumnWidth =
          columnWidth != null
            ? columnWidth
            : layoutEntry.w > 0 && widthPx > 0
              ? widthPx / layoutEntry.w
              : null;
        const widthEstimate =
          fallbackColumnWidth != null
            ? Math.round(fallbackColumnWidth * layoutEntry.w * 1000) / 1000
            : widthPx;
      const heightEstimate = Math.max(rowHeightPx * layoutEntry.h, heightPx);
        if (
          Number.isFinite(widthEstimate) &&
          Number.isFinite(heightEstimate) &&
          widthEstimate > 0 &&
          heightEstimate > 0
        ) {
          if (typeof window !== 'undefined') {
            try {
            tileDebugLog('[tile-diag] manual-measure', {
              id: newItem.i,
              widthPx: widthEstimate,
              heightPx: heightEstimate,
              columnWidth: fallbackColumnWidth,
              layoutUnits: { w: layoutEntry.w, h: layoutEntry.h },
            });
            } catch {
              // ignore logging issues
            }
          }
          sessionTileController.updateTileViewState(
            newItem.i,
            'view-state.resize.measurement',
            {
              measurements: {
                width: widthEstimate,
                height: heightEstimate,
              },
            },
            { persist: false },
          );
        }
      }
      if (state.locked) {
        scheduleHostResize(newItem.i);
      }
    },
    [clampLayoutItems, cols, gridWidth, handleLayoutChange, handleLayoutCommit, rowHeightPx, scheduleHostResize, viewStateMap],
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
      if (!layoutItem || gridWidth == null || gridWidth <= 0 || cols <= 0 || layoutItem.w <= 0) {
        return;
      }
      const state = viewStateMap[sessionId];
      if (!state || !state.manualLayout) {
        return;
      }
      const columnWidth = getColumnWidth(gridWidth, cols);
      if (columnWidth == null) {
        return;
      }
      const layoutWidth =
        columnWidth * layoutItem.w;
      const layoutHeight = Math.max(rowHeightPx * layoutItem.h, 1);
      if (!Number.isFinite(layoutWidth) || layoutWidth <= 0) {
        return;
      }
      const normalized: TileMeasurements = {
        width: layoutWidth,
        height: layoutHeight,
      };
      const existing = viewStateMap[sessionId]?.measurements;
      if (existing && isSameMeasurement(existing, normalized)) {
        return;
      }
      sessionTileController.updateTileViewState(
        sessionId,
        'view-state.manual.measurement',
        {
          measurements: normalized,
        },
        { persist: false },
      );
      tileDebugLog(
        '[tile-layout] measure',
        JSON.stringify(
          {
            sessionId,
            width: normalized.width,
            height: normalized.height,
            rawWidth: measurement.width,
            rawHeight: measurement.height,
            gridWidth,
            cols,
            layoutItem,
            columnWidth,
            layoutHeight,
          },
          (_key, value) => (value instanceof Map ? undefined : value),
        ),
      );
    },
    [cols, gridWidth, layoutMap, rowHeightPx, viewStateMap],
  );

  const handlePreviewStatusChange = useCallback(
    (sessionId: string, status: 'connecting' | 'initializing' | 'ready' | 'error') => {
      sessionTileController.setTilePreviewStatus(sessionId, status);
    },
    [],
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
      tileDebugLog('[tile-layout] viewport-payload', {
        version: 'v2',
        sessionId,
        viewportRows: dims?.viewportRows ?? null,
        viewportCols: dims?.viewportCols ?? null,
        hostRows: dims?.hostRows ?? null,
        hostCols: dims?.hostCols ?? null,
      });
      if (
        !dims ||
        typeof dims.viewportRows !== 'number' ||
        typeof dims.viewportCols !== 'number' ||
        dims.viewportRows <= 0 ||
        dims.viewportCols <= 0
      ) {
        tileDebugWarn('[tile-layout] viewport-dims skipped', { sessionId, dims });
        return;
      }
      tileDebugLog('[tile-layout] viewport-dims raw', JSON.stringify(dims));
      tileDebugLog('[tile-layout] viewport-dims', {
        version: 'v1',
        sessionId,
        viewportRows: dims.viewportRows,
        viewportCols: dims.viewportCols,
        hostRows: dims.hostRows,
        hostCols: dims.hostCols,
      });
      const state = viewStateMap[sessionId] ?? defaultTileViewState();
      const viewportRows = dims.viewportRows > 0 ? dims.viewportRows : null;
      const viewportCols = dims.viewportCols > 0 ? dims.viewportCols : null;
      const hostRowsCandidate =
        typeof dims.hostRows === 'number' && dims.hostRows > 0 ? dims.hostRows : null;
      const hostColsCandidate =
        typeof dims.hostCols === 'number' && dims.hostCols > 0 ? dims.hostCols : null;
      const hostProvided = hostRowsCandidate != null || hostColsCandidate != null;

      const resolvedHostRows = (() => {
        if (hostRowsCandidate != null) {
          return hostRowsCandidate;
        }
        if (state.hostRows && state.hostRows > 0) {
          return state.hostRows;
        }
        if (viewportRows && viewportRows > 0) {
          return viewportRows;
        }
        return DEFAULT_HOST_ROWS;
      })();

      const resolvedHostCols = (() => {
        if (hostColsCandidate != null) {
          return hostColsCandidate;
        }
        if (state.hostCols && state.hostCols > 0) {
          return state.hostCols;
        }
        if (viewportCols && viewportCols > 0) {
          return viewportCols;
        }
        return DEFAULT_HOST_COLS;
      })();

      const cappedViewportRows =
        viewportRows && viewportRows > 0
          ? Math.min(viewportRows, resolvedHostRows)
          : state.viewportRows && state.viewportRows > 0
            ? Math.min(state.viewportRows, resolvedHostRows)
            : resolvedHostRows;
      const cappedViewportCols =
        viewportCols && viewportCols > 0
          ? Math.min(viewportCols, resolvedHostCols)
          : state.viewportCols && state.viewportCols > 0
            ? Math.min(state.viewportCols, resolvedHostCols)
            : resolvedHostCols;

      const nextHostRows = Math.max(DEFAULT_HOST_ROWS, resolvedHostRows);
      const nextHostCols = Math.max(DEFAULT_HOST_COLS, resolvedHostCols);
      const nextViewportRows = Math.max(DEFAULT_HOST_ROWS, cappedViewportRows);
      const nextViewportCols = Math.max(DEFAULT_HOST_COLS, cappedViewportCols);
      const nextState: TileViewState = {
        ...state,
        viewportRows: nextViewportRows,
        viewportCols: nextViewportCols,
        hostRows: nextHostRows,
        hostCols: nextHostCols,
        hasHostDimensions: state.hasHostDimensions || hostProvided,
      };
        tileDebugLog('[tile-layout] viewport-apply', {
          sessionId,
          prevViewportRows: state.viewportRows,
          nextViewportRows,
          prevViewportCols: state.viewportCols,
          nextViewportCols,
          prevHostRows: state.hostRows,
          nextHostRows,
          prevHostCols: state.hostCols,
          nextHostCols,
          resolvedHostRows,
          resolvedHostCols,
          cappedViewportRows,
          cappedViewportCols,
        });
      const patch = diffTileViewState(state, nextState);
      if (patch) {
        sessionTileController.updateTileViewState(sessionId, 'view-state.viewport', patch, { persist: false });
      }
    },
    [viewStateMap],
  );

  const handleSnap = useCallback(
    (sessionId: string) => {
      const layoutItem = layoutMap.get(sessionId);
      const state = viewStateMap[sessionId];
      const measurement = state?.measurements;
      if (!layoutItem || !state || !measurement) {
        sessionTileController.updateTileViewState(sessionId, 'view-state.snap', {
          locked: false,
          zoom: DEFAULT_ZOOM,
        });
        return;
      }
      const effectiveCols = (() => {
        const viewport = state.viewportCols && state.viewportCols > 0 ? state.viewportCols : null;
        const host = state.hostCols && state.hostCols > 0 ? state.hostCols : null;
        if (viewport != null && host != null) {
          return Math.min(host, viewport);
        }
        return viewport ?? host;
      })();
      const effectiveRows = (() => {
        const viewport = state.viewportRows && state.viewportRows > 0 ? state.viewportRows : null;
        const host = state.hostRows && state.hostRows > 0 ? state.hostRows : null;
        if (viewport != null && host != null) {
          return Math.min(host, viewport);
        }
        return viewport ?? host;
      })();
      const hostSize = estimateHostSize(effectiveCols, effectiveRows);
      const targetHostWidth = Math.min(hostSize.width, MAX_TILE_WIDTH_PX);
      const targetHostHeight = Math.min(hostSize.height, MAX_TILE_HEIGHT_PX);
      const columnWidth = getColumnWidth(gridWidth, cols);
      const unitWidth = (() => {
        if (columnWidth != null) {
          return Math.max(1, columnWidth);
        }
        if (layoutItem.w > 0) {
          return Math.max(1, measurement.width / layoutItem.w);
        }
        return Math.max(1, measurement.width);
      })();
      const unitHeight = rowHeightPx;
      const targetWUnits = Math.max(MIN_W, Math.round(targetHostWidth / unitWidth));
      const targetHUnits = Math.max(MIN_H, Math.round(targetHostHeight / unitHeight));
      const currentWidthUnits = Math.max(MIN_W, Math.min(layoutItem.w, cols));
      const clampedWidth = Math.min(targetWUnits, cols);
      const nextWidth = Math.min(clampedWidth, currentWidthUnits);
      const nextLayouts = clampLayoutItems(
        layout.map((item) =>
          item.i === sessionId
            ? {
                ...item,
                w: nextWidth,
                h: targetHUnits,
              }
            : item,
        ),
        cols,
      );
      handleLayoutChange(nextLayouts);
      handleLayoutCommit(nextLayouts, 'state-change');
      const applied = nextLayouts.find((item) => item.i === sessionId);
      const spansFullWidth = Boolean(applied && applied.w >= cols);
      const nextState: TileViewState = {
        ...state,
        locked: false,
        zoom: clampZoom(spansFullWidth ? MAX_UNLOCKED_ZOOM : DEFAULT_ZOOM),
        manualLayout: false,
        layoutHostCols: state.hostCols ?? state.layoutHostCols,
        layoutHostRows: state.hostRows ?? state.layoutHostRows,
      };
      const patch = diffTileViewState(state, nextState);
      if (patch) {
        sessionTileController.updateTileViewState(sessionId, 'view-state.snap', patch);
      }
    },
    [clampLayoutItems, cols, gridWidth, handleLayoutChange, handleLayoutCommit, layout, layoutMap, rowHeightPx, viewStateMap],
  );

  const handleToggleLock = useCallback(
    (sessionId: string) => {
      const snapshot = sessionTileController.getTileSnapshot(sessionId);
      const current = selectTileViewState(snapshot.grid);
      const nextLocked = !current.locked;
      const nextZoom = nextLocked ? MAX_UNLOCKED_ZOOM : clampZoom(current.zoom);
      sessionTileController.updateTileViewState(sessionId, 'view-state.lock.toggle', {
        locked: nextLocked,
        zoom: nextZoom,
      });
    },
    [],
  );

  const handleToolbarToggle = useCallback(
    (sessionId: string) => {
      const snapshot = sessionTileController.getTileSnapshot(sessionId);
      const current = selectTileViewState(snapshot.grid);
      const nextPinned = !current.toolbarPinned;
      sessionTileController.setTileToolbarPinned(sessionId, nextPinned);
    },
    [],
  );

  const toggleAssignments = useCallback((sessionId: string) => {
    setCollapsedAssignments((prev) => {
      const next = { ...prev };
      const current = prev[sessionId] ?? true;
      next[sessionId] = !current;
      return next;
    });
  }, []);

  useLayoutEffect(() => {
    normalizeSavedLayout();
  }, [normalizeSavedLayout]);

  const gridContent = isClient ? (
    <div className="session-grid">
      <AutoGrid
        layout={layoutForRender}
        cols={cols}
        rowHeight={rowHeightPx}
        margin={[GRID_MARGIN_X, GRID_MARGIN_Y]}
        containerPadding={[GRID_CONTAINER_PADDING_X, GRID_CONTAINER_PADDING_Y]}
        compactType={null}
        preventCollision={false}
        draggableHandle=".session-tile-drag-grip"
        draggableCancel=".session-tile-actions"
        onDragStart={handleDragStart}
        resizeHandle={renderResizeHandle}
        resizeHandles={['e', 's', 'se']}
        onDragStop={handleDragStop}
        onResizeStop={handleResizeStop}
        innerRef={setGridElementNode}
        onLayoutChange={handleLayoutChange}
      >
        {tiles.map((session) => {
          const role = roles.get(session.session_id) ?? 'application';
          const isAgent = role === 'agent';
          const agentAssignments = assignmentsByAgent.get(session.session_id) ?? [];
          const controllers = assignmentsByApplication.get(session.session_id) ?? [];
          const collapsed = collapsedAssignments[session.session_id] ?? true;
          const resizeControl = resizeControls[session.session_id];
          const isExpanded = expanded?.session_id === session.session_id;

          return (
            <SessionTile
              key={session.session_id}
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
              onMeasure={(measurement) => handleMeasure(session.session_id, measurement)}
              onViewport={handleViewportDimensions}
              onPreviewStatusChange={handlePreviewStatusChange}
              onPreviewMeasurementsChange={handleTilePreviewMeasurementsChange}
              onHostResizeStateChange={handleHostResizeStateChange}
              isExpanded={isExpanded}
              className="session-grid-item"
              viewerOverride={effectiveViewerOverrides[session.session_id] ?? null}
              cachedDiff={cachedTerminalDiffs[session.session_id] ?? null}
            />
          );
        })}
      </AutoGrid>
    </div>
  ) : (
    <div className="h-[520px] rounded-xl border border-border bg-card shadow-sm" />
  );

  useEffect(() => {
    if (!TILE_DEBUG_ENABLED || typeof window === 'undefined') return;
    tileDebugLog('[tile-layout] instrumentation', { component: 'TileCanvas', version: 'v1' });
  }, []);

  useEffect(() => {
    if (!TILE_DEBUG_ENABLED || !isClient) return;
    const wrapper = gridWrapperRef.current;
    if (!wrapper) return;
    const target =
      gridElementNode ??
      wrapper.querySelector<HTMLElement>('.react-grid-layout') ??
      wrapper.parentElement?.querySelector<HTMLElement>('.react-grid-layout') ??
      wrapper;
    if (!target) {
      tileDebugLog('[tile-layout] dom-log skipped', { version: 'v1', layoutSignature });
      return;
    }
    tileDebugLog('[tile-layout] dom-log start', {
      version: 'v1',
      layoutSignature,
      hasGrid: Boolean(gridElementNode),
      targetClass: target.className,
    });
    const logItems = (phase: 'initial' | 'mutation'): boolean => {
      const items = target.querySelectorAll<HTMLElement>('.react-grid-item');
      if (items.length === 0) {
        const childSummaries = Array.from(target.children)
          .slice(0, 3)
          .map((el) => ({ tag: el.tagName, className: el.className, dataAttrs: { ...el.dataset } }));
        tileDebugLog(
          '[tile-layout] dom-item pending',
          JSON.stringify({
            version: 'v1',
            phase,
            layoutSignature,
            count: items.length,
            childSummaries,
            htmlSample: target.innerHTML.slice(0, 200),
          }),
        );
        return false;
      }
      items.forEach((item) => {
        const sessionId = item.dataset.sessionId ?? item.getAttribute('data-grid-session') ?? 'unknown';
        const rect = item.getBoundingClientRect();
        tileDebugLog(
          '[tile-layout] dom-item',
          JSON.stringify({
            version: 'v1',
            sessionId,
            width: Math.round(rect.width * 100) / 100,
            height: Math.round(rect.height * 100) / 100,
          }),
        );
      });
      return true;
    };
    if (logItems('initial')) {
      return;
    }
    const observer = new MutationObserver((mutations) => {
      for (const mutation of mutations) {
        mutation.addedNodes.forEach((node) => {
          if (node instanceof HTMLElement && node.classList.contains('react-grid-item')) {
            tileDebugLog('[tile-layout] dom-mutation', {
              version: 'v1',
              tag: node.tagName,
              className: node.className,
              dataset: { ...node.dataset },
              style: node.getAttribute('style'),
            });
          }
        });
      }
      if (logItems('mutation')) {
        observer.disconnect();
      }
    });
    observer.observe(target, { childList: true, subtree: true });
    return () => observer.disconnect();
  }, [gridElementNode, isClient, layoutSignature]);

  useEffect(() => {
    if (!TILE_DEBUG_ENABLED || !isClient) return;
    const wrapper = gridWrapperRef.current;
    if (!wrapper) return;
    const firstItem = wrapper.querySelector<HTMLElement>('.react-grid-item');
    if (firstItem) {
      const rect = firstItem.getBoundingClientRect();
      tileDebugLog(
        '[tile-layout] item-width',
        JSON.stringify({ width: rect.width, height: rect.height }),
      );
    } else {
      tileDebugLog('[tile-layout] item-width', 'missing react-grid-item');
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
            <ExpandedSessionPreview
              session={expanded}
              managerUrl={managerUrl}
              viewerToken={viewerToken}
              onHostResizeStateChange={handleHostResizeStateChange}
              onViewportDimensions={handleViewportDimensions}
              onPreviewMeasurementsChange={handleTilePreviewMeasurementsChange}
              onPreviewStatusChange={(status) => {
                sessionTileController.setTilePreviewStatus(expanded.session_id, status);
              }}
            />
          </div>
        </div>
      )}
    </div>
  );
}
