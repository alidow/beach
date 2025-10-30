import {
  forwardRef,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from 'react';
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
import { useSessionTerminal, type TerminalViewerState } from '../hooks/useSessionTerminal';
import { emitTelemetry } from '../lib/telemetry';
import { extractSessionTitle } from '../lib/sessionMetadata';
import { buildViewerStateFromTerminalDiff, extractTerminalStateDiff } from '../lib/terminalHydrator';

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
const LEGACY_GRID_COLS = 12;
const LEGACY_ROW_HEIGHT_PX = 110;
const CROPPED_EPSILON = 0.02;

type LayoutCache = Record<string, Layout>;

type TileMeasurements = {
  width: number;
  height: number;
};

type PreviewMetrics = {
  scale: number;
  targetWidth: number;
  targetHeight: number;
  rawWidth: number;
  rawHeight: number;
  hostRows: number | null;
  hostCols: number | null;
  measurementVersion: number;
};

type TileViewState = {
  zoom: number;
  locked: boolean;
  toolbarPinned: boolean;
  measurements: TileMeasurements | null;
  hostCols: number | null;
  hostRows: number | null;
  hasHostDimensions: boolean;
  viewportCols: number | null;
  viewportRows: number | null;
  lastLayout: { w: number; h: number } | null;
  layoutInitialized: boolean;
  manualLayout: boolean;
  layoutHostCols: number | null;
  layoutHostRows: number | null;
  previewStatus: 'connecting' | 'initializing' | 'ready' | 'error';
  preview: PreviewMetrics | null;
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

function normalizeSavedLayoutItem(item: BeachLayoutItem, cols: number): BeachLayoutItem {
  const sourceCols =
    item.gridCols && item.gridCols > 0 ? item.gridCols : LEGACY_GRID_COLS;
  const sourceRowHeight =
    item.rowHeightPx && item.rowHeightPx > 0 ? item.rowHeightPx : LEGACY_ROW_HEIGHT_PX;
  const hasTargetCols = sourceCols === cols;
  const hasTargetRows = sourceRowHeight === ROW_HEIGHT;
  if (hasTargetCols && hasTargetRows && (item.layoutVersion ?? 0) >= GRID_LAYOUT_VERSION) {
    return item;
  }
  const colScale = cols / Math.max(1, sourceCols);
  const rowScale = ROW_HEIGHT / Math.max(1, sourceRowHeight);
  const scaledW = Math.max(MIN_W, Math.round(item.w * colScale));
  const scaledH = Math.max(MIN_H, Math.round(item.h * rowScale));
  const scaledX = Math.max(0, Math.round(item.x * colScale));
  const scaledY = Math.max(0, Math.round(item.y * rowScale));
  const clampedW = Math.min(cols, scaledW);
  const clampedX = Math.max(0, Math.min(scaledX, Math.max(0, cols - clampedW)));
  return {
    ...item,
    x: clampedX,
    y: scaledY,
    w: clampedW,
    h: scaledH,
    gridCols: cols,
    rowHeightPx: ROW_HEIGHT,
    layoutVersion: GRID_LAYOUT_VERSION,
  };
}

function buildTileState(saved?: BeachLayoutItem): TileViewState {
  const normalizedSaved = saved ? normalizeSavedLayoutItem(saved, DEFAULT_COLS) : undefined;
  const locked = Boolean(saved?.locked);
  const savedMeasurement =
    normalizedSaved?.widthPx && normalizedSaved?.heightPx
      ? { width: normalizedSaved.widthPx, height: normalizedSaved.heightPx }
      : null;
  const measurement =
    !locked && savedMeasurement && savedMeasurement.width > UNLOCKED_MEASUREMENT_LIMIT
      ? null
      : savedMeasurement;
  const estimatedZoom = measurement
    ? computeZoomForSize(
        measurement,
        normalizedSaved?.hostCols ?? null,
        normalizedSaved?.hostRows ?? null,
        null,
        null,
      )
    : DEFAULT_ZOOM;
  const baselineZoom = locked
    ? MAX_UNLOCKED_ZOOM
    : clampZoom(normalizedSaved?.zoom ?? estimatedZoom, measurement);
  const zoom = baselineZoom;
  const baseline: TileViewState = {
    zoom,
    locked,
    toolbarPinned: Boolean(normalizedSaved?.toolbarPinned),
    measurements: measurement,
    preview: null,
    hostCols: null,
    hostRows: null,
    hasHostDimensions:
      typeof normalizedSaved?.hostCols === 'number' && normalizedSaved.hostCols > 0
        ? true
        : typeof normalizedSaved?.hostRows === 'number' && normalizedSaved.hostRows > 0,
    viewportCols: null,
    viewportRows: null,
    lastLayout: null,
    layoutInitialized: false,
    manualLayout: Boolean(normalizedSaved),
    layoutHostCols: null,
    layoutHostRows: null,
    previewStatus: 'connecting',
  };
  if (normalizedSaved) {
    const { w, h } = clampGridSize(normalizedSaved.w, normalizedSaved.h, baseline, DEFAULT_COLS, true);
    baseline.lastLayout = { w, h };
    if (typeof normalizedSaved.hostCols === 'number' && normalizedSaved.hostCols > 0) {
      baseline.hostCols = normalizedSaved.hostCols;
    }
    if (typeof normalizedSaved.hostRows === 'number' && normalizedSaved.hostRows > 0) {
      baseline.hostRows = normalizedSaved.hostRows;
    }
  }
  return baseline;
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
    const normalized = normalizeSavedLayoutItem(item, effectiveCols);
    const w = Math.min(effectiveCols, Math.max(MIN_W, Math.floor(normalized.w)));
    const h = Math.max(MIN_H, Math.floor(normalized.h));
    const x = Math.max(0, Math.min(Math.floor(normalized.x), effectiveCols - w));
    const y = Math.max(0, Math.floor(normalized.y));
    savedMap.set(item.id, { ...normalized, x, y, w, h });
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
  if (typeof window !== 'undefined') {
    try {
      console.info(
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
      console.info('[tile-layout] tile-zoom', {
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

  const handleTilePreviewStatusChange = (
    sessionId: string,
    status: 'connecting' | 'initializing' | 'ready' | 'error',
  ) => {
    updateTileState(sessionId, (state) => {
      if (state.previewStatus === status) {
        return state;
      }
      return {
        ...state,
        previewStatus: status,
      };
    });
  };

  const handlePreviewStatusChange = useCallback(
    (status: 'connecting' | 'initializing' | 'ready' | 'error') => {
      onPreviewStatusChange(session.session_id, status);
    },
    [onPreviewStatusChange, session.session_id],
  );

  if (typeof window !== 'undefined') {
    try {
      console.info('[tile-layout] render-state', {
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
    } catch {
      // ignore logging issues
    }
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
              viewerOverride={viewer}
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
  onPreviewMeasurementsChange: (sessionId: string, measurement: PreviewMetrics | null) => void;
  onHostResizeStateChange: (sessionId: string, state: HostResizeControlState | null) => void;
  onViewerStateChange: (sessionId: string, viewer: TerminalViewerState | null) => void;
  isExpanded: boolean;
  className?: string;
  style?: CSSProperties;
  viewerOverride?: TerminalViewerState | null;
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
      view,
      onMeasure,
      onViewport,
      onPreviewStatusChange,
      onPreviewMeasurementsChange,
      onHostResizeStateChange,
      onViewerStateChange,
      isExpanded,
      className,
      style,
      viewerOverride,
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
      },
      [onPreviewMeasurementsChange],
    );

  const trimmedToken = viewerToken?.trim() ?? '';
  const effectiveOverride = viewerOverride ?? null;
  const shouldUseLiveViewer = effectiveOverride == null;
  const viewer = useSessionTerminal(
    shouldUseLiveViewer ? session.session_id : null,
    shouldUseLiveViewer ? session.private_beach_id : null,
    managerUrl,
    shouldUseLiveViewer && trimmedToken.length > 0 ? trimmedToken : null,
  );

  const effectiveViewer = effectiveOverride ?? viewer;

  const sessionName = useMemo(() => extractSessionTitle(session.metadata), [session.metadata]);
  const shortSessionId = useMemo(() => session.session_id.slice(0, 8), [session.session_id]);
  const dragGripClass = sessionName
    ? 'pointer-events-auto session-tile-drag-grip flex min-w-0 flex-col items-start gap-1 text-left text-muted-foreground'
    : 'pointer-events-auto session-tile-drag-grip flex items-center gap-2 text-[10px] uppercase tracking-[0.36em] text-muted-foreground';

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

  const viewerSnapshot = useMemo<TerminalViewerState>(() => {
    return {
      store: effectiveViewer.store,
      transport: effectiveViewer.transport,
      transportVersion: effectiveViewer.transportVersion,
      connecting: effectiveViewer.connecting,
      error: effectiveViewer.error,
      status: effectiveViewer.status,
      secureSummary: effectiveViewer.secureSummary,
      latencyMs: effectiveViewer.latencyMs,
    };
  }, [
    effectiveViewer.store,
    effectiveViewer.transport,
    effectiveViewer.transportVersion,
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
      shouldUseLiveViewer,
      hasOverride: Boolean(effectiveOverride),
      manualLayout: view.manualLayout,
      locked: view.locked,
    };
    const signature = JSON.stringify(payload);
    if (viewerConfigSummaryRef.current !== signature) {
      viewerConfigSummaryRef.current = signature;
      console.info('[tile-viewer] config', payload);
    }
  }, [effectiveOverride, session.session_id, shouldUseLiveViewer, view.locked, view.manualLayout]);

  useEffect(() => {
    const transportType =
      viewerSnapshot.transport?.constructor?.name ??
      (viewerSnapshot.transport ? 'custom' : null);
    if (typeof window !== 'undefined') {
      console.info('[tile-viewer] snapshot', {
        sessionId: session.session_id,
        status: viewerSnapshot.status,
        connecting: viewerSnapshot.connecting,
        latencyMs: viewerSnapshot.latencyMs,
        transportType,
        transportVersion: viewerSnapshot.transportVersion,
        hasStore: Boolean(viewerSnapshot.store),
      });
    }
    onViewerStateChange(session.session_id, viewerSnapshot);
    return () => {
      if (typeof window !== 'undefined') {
        console.info('[tile-viewer] snapshot-dispose', {
          sessionId: session.session_id,
        });
      }
      onViewerStateChange(session.session_id, null);
    };
  }, [
    onViewerStateChange,
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
    if (typeof window !== 'undefined') {
      try {
        console.info('[tile-diag] session-tile mount', {
          sessionId: session.session_id,
          viewerTokenProvided: Boolean(viewerToken),
        });
      } catch {
        // ignore logging issues
      }
    }
    return () => {
      if (typeof window !== 'undefined') {
        try {
          console.info('[tile-diag] session-tile unmount', {
            sessionId: session.session_id,
          });
        } catch {
          // ignore logging issues
        }
      }
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
  viewerOverrides?: Partial<Record<string, TerminalViewerState>>;
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
  viewerOverrides,
}: Props) {
  const [cache, setCache] = useState<LayoutCache>({});
  const [expanded, setExpanded] = useState<SessionSummary | null>(null);
  const [isClient, setIsClient] = useState(false);
  const [collapsedAssignments, setCollapsedAssignments] = useState<Record<string, boolean>>({});
  const [resizeControls, setResizeControls] = useState<Record<string, HostResizeControlState>>({});
  const [viewerStates, setViewerStates] = useState<Record<string, TerminalViewerState>>({});
  const [cols, setCols] = useState(DEFAULT_COLS);
  const [gridWidth, setGridWidth] = useState<number | null>(null);
  const [gridElementNode, setGridElementNode] = useState<HTMLElement | null>(null);
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

  const autoViewerOverrides = useMemo<Record<string, TerminalViewerState>>(() => {
    if (!tiles || tiles.length === 0) {
      return {};
    }
    const overrides: Record<string, TerminalViewerState> = {};
    for (const session of tiles) {
      try {
        const diff = extractTerminalStateDiff(session.metadata);
        if (!diff) {
          continue;
        }
        const viewerState = buildViewerStateFromTerminalDiff(diff);
        if (viewerState) {
          overrides[session.session_id] = viewerState;
        }
      } catch (err) {
        console.warn('[tile-canvas] terminal preview hydration failed', {
          sessionId: session.session_id,
          error: err instanceof Error ? err.message : err,
        });
      }
    }
    return overrides;
  }, [tiles]);

  const effectiveViewerOverrides = useMemo<Record<string, TerminalViewerState | null | undefined>>(() => {
    if (!viewerOverrides || Object.keys(viewerOverrides).length === 0) {
      return autoViewerOverrides;
    }
    return {
      ...autoViewerOverrides,
      ...viewerOverrides,
    };
  }, [autoViewerOverrides, viewerOverrides]);

  const tileStateRef = useRef<TileStateMap>(tileState);
  const prevTileStateRef = useRef<TileStateMap>({});
  const resizeControlRef = useRef<Record<string, HostResizeControlState>>(resizeControls);
  const gridWrapperRef = useRef<HTMLDivElement | null>(null);
  const lastPersistSignatureRef = useRef<string | null>(null);
  const autoSizingRef = useRef<Set<string>>(new Set());

  const computeCols = useCallback(() => {
    return DEFAULT_COLS;
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

  const handleViewerStateChange = useCallback(
    (sessionId: string, viewer: TerminalViewerState | null) => {
      setViewerStates((prev) => {
        const existing = prev[sessionId];
        if (!viewer) {
          if (existing === undefined) {
            return prev;
          }
          const next = { ...prev };
          delete next[sessionId];
          return next;
        }
        if (typeof window !== 'undefined') {
          try {
            console.info('[tile-diag] viewer-state-change', {
              sessionId,
              status: viewer.status,
              transport: viewer.transport ? viewer.transport.constructor?.name ?? 'custom' : null,
            });
          } catch {
            // ignore logging issues
          }
        }
        if (
          existing &&
          existing.store === viewer.store &&
          existing.transport === viewer.transport &&
          existing.status === viewer.status &&
          existing.error === viewer.error &&
          existing.secureSummary === viewer.secureSummary &&
          existing.latencyMs === viewer.latencyMs &&
          existing.connecting === viewer.connecting
        ) {
          return prev;
        }
        return {
          ...prev,
          [sessionId]: viewer,
        };
      });
    },
    [],
  );

  useEffect(() => {
    if (!isClient) {
      return undefined;
    }
    const applyWidth = (width: number | null) => {
      const fallback = typeof window !== 'undefined' ? window.innerWidth : TARGET_TILE_WIDTH;
      const targetWidth = width ?? fallback ?? TARGET_TILE_WIDTH;
      if (typeof window !== 'undefined') {
        try {
          console.info('[tile-diag] apply-width', { width, fallback, targetWidth });
        } catch {
          // ignore
        }
      }
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
        const normalized = normalizeSavedLayoutItem(item, cols);
        const current = next[item.id] ?? buildTileState(normalized);
        const merged: TileViewState = {
          ...current,
          locked: typeof normalized.locked === 'boolean' ? normalized.locked : current.locked,
          toolbarPinned:
            typeof normalized.toolbarPinned === 'boolean'
              ? normalized.toolbarPinned
              : current.toolbarPinned,
          lastLayout: null,
          layoutInitialized: true,
          manualLayout: true,
          hasHostDimensions:
            current.hasHostDimensions ||
            (typeof normalized.hostCols === 'number' && normalized.hostCols > 0) ||
            (typeof normalized.hostRows === 'number' && normalized.hostRows > 0),
        };
        const initialSize = clampGridSize(normalized.w, normalized.h, merged, cols, true);
        merged.lastLayout = initialSize;
        if (normalized.widthPx && normalized.heightPx) {
          merged.measurements = { width: normalized.widthPx, height: normalized.heightPx };
        }
        if (typeof normalized.hostCols === 'number' && normalized.hostCols > 0) {
          merged.hostCols = normalized.hostCols;
        }
        if (typeof normalized.hostRows === 'number' && normalized.hostRows > 0) {
          merged.hostRows = normalized.hostRows;
        }
        if (!merged.locked) {
          merged.zoom = clampZoom(normalized.zoom ?? merged.zoom);
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
      const next: Layout = { ...item, maxW: cols };
      delete next.maxH; // allow taller tiles so snap-to-host can match large hosts
      return next;
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
  const layoutSignature = useMemo(() => {
    return JSON.stringify(layout.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })));
  }, [layout]);

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

const tileOrder = useMemo(() => tiles.map((t) => t.session_id), [tiles]);

  useEffect(() => {
    if (!isClient || !gridWidth || gridWidth <= 0 || cols <= 0) {
      return;
    }
    const columnWidth = getColumnWidth(gridWidth, cols);
    if (columnWidth == null) {
      return;
    }
    if (typeof window !== 'undefined') {
      try {
        console.info('[tile-diag] autosize-start', {
          cols,
          gridWidth,
          columnWidth,
          tileCount: layout.length,
        });
      } catch {
        // ignore logging issues
      }
    }
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
      const state = tileState[item.i];
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
          MIN_H * ROW_HEIGHT,
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
          MIN_H * ROW_HEIGHT,
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
        const raw = heightPx / Math.max(ROW_HEIGHT, 1e-6);
        return Math.max(MIN_H, Math.ceil(raw));
      };
      const targetW = computeWidthUnits(targetWidthPx);
      const targetH = computeHeightUnits(targetHeightPx);
      const normalizedWidthPx = targetW * columnWidth;
      const normalizedHeightPx = targetH * ROW_HEIGHT;
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
      if (typeof window !== 'undefined') {
        try {
          console.info('[tile-diag] autosize-eval-detail', {
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
        } catch {
          // ignore logging issues
        }
      }
    });
    if (typeof window !== 'undefined') {
      try {
        console.info('[tile-diag] autosize-evaluated', {
          adjustments,
          initializedTiles,
        });
      } catch {
        // ignore logging issues
      }
    }
    if (adjustments.length === 0) {
      autoSizingRef.current.clear();
      if (initializedTiles.length > 0) {
        setTileState((prev) => {
          let changed = false;
          const next: TileStateMap = { ...prev };
          initializedTiles.forEach(({ id, hostCols, hostRows, widthPx, heightPx }) => {
            const current = next[id];
            if (
              current &&
              (!current.layoutInitialized ||
                current.layoutHostCols !== hostCols ||
                current.layoutHostRows !== hostRows ||
                current.manualLayout ||
                !current.hasHostDimensions ||
                !current.measurements ||
                !isSameMeasurement(current.measurements, { width: widthPx, height: heightPx }))
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
              next[id] = {
                ...current,
                layoutInitialized: true,
                manualLayout: false,
                layoutHostCols: hostCols,
                layoutHostRows: hostRows,
                hasHostDimensions: true,
                measurements: hasPreview ? current.measurements : { width: widthPx, height: heightPx },
                zoom: current.locked ? MAX_UNLOCKED_ZOOM : hasPreview ? current.zoom : autoZoom,
              };
              if (typeof window !== 'undefined') {
                try {
                  console.info('[tile-diag] autosize-initialize', {
                    id,
                    hostCols,
                    hostRows,
                    widthPx,
                    heightPx,
                    zoom: current.locked ? MAX_UNLOCKED_ZOOM : hasPreview ? current.zoom : autoZoom,
                    reason: 'alreadySized',
                  });
                } catch {
                  // ignore logging issues
                }
              }
              changed = true;
            }
          });
          return changed ? next : prev;
        });
      }
      return;
    }
    autoSizingRef.current = new Set(adjustments.map(({ id }) => id));
    setCache((prev) => {
      const next: LayoutCache = { ...prev };
      adjustments.forEach(({ id, w, h }) => {
        const normalized = clampGridSize(w, h, tileState[id], cols, false);
        const existing = layoutMap.get(id);
        next[id] = {
          ...(existing ?? { i: id, x: 0, y: 0, minW: MIN_W, minH: MIN_H }),
          w: normalized.w,
          h: normalized.h,
          minW: MIN_W,
          minH: MIN_H,
          maxW: cols,
        };
      });
      return next;
    });
    setTileState((prev) => {
      let changed = false;
      const next: TileStateMap = { ...prev };
      adjustments.forEach(({ id, w, h, hostCols, hostRows, widthPx, heightPx }) => {
        const normalized = clampGridSize(w, h, next[id], cols, false);
        const current = next[id] ?? buildTileState();
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
        next[id] = {
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
        if (typeof window !== 'undefined') {
          try {
            console.info('[tile-diag] autosize-apply', {
              id,
              hostCols,
              hostRows,
              widthPx,
              heightPx,
              gridWidth,
              cols,
              gridUnits: { w: normalized.w, h: normalized.h },
              zoom: current.locked ? MAX_UNLOCKED_ZOOM : hasPreview ? current.zoom : autoZoom,
            });
          } catch {
            // ignore logging issues
          }
        }
        changed = true;
      });
      return changed ? next : prev;
    });
  }, [cols, gridWidth, isClient, layout, layoutMap, tileState]);
  const clampLayoutItems = useCallback(
    (layouts: Layout[], colsValue: number): Layout[] => {
      const effectiveCols = Math.max(DEFAULT_W, colsValue || DEFAULT_COLS);
      const stateMap = tileStateRef.current;
      return layouts.map((item) => {
        const state = stateMap[item.i];
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
        entry.gridCols = effectiveCols;
        entry.rowHeightPx = ROW_HEIGHT;
        entry.layoutVersion = GRID_LAYOUT_VERSION;
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
      try {
        emitTelemetry('canvas.layout.persist', { reason, tiles: normalized.length });
      } catch {}
    },
    [clampLayoutItems, cols, onLayoutPersist, snapshotLayout],
  );

  const handleLayoutChange = useCallback(
    (nextLayouts: Layout[]) => {
      if (typeof window !== 'undefined') {
        console.info(
          '[tile-layout] onLayoutChange',
          JSON.stringify(nextLayouts.map(({ i, x, y, w, h }) => ({ i, x, y, w, h }))),
        );
      }
      const normalized = clampLayoutItems(nextLayouts, cols);
      const autoSizingIds = autoSizingRef.current;
      const isAutosizingEvent =
        normalized.length > 0 && normalized.every((item) => autoSizingIds.has(item.i));
      if (isAutosizingEvent) {
        if (typeof window !== 'undefined') {
          console.info(
            '[tile-layout] onLayoutChange autosize-skip',
            JSON.stringify(normalized.map(({ i, x, y, w, h }) => ({ i, x, y, w, h }))),
          );
        }
        normalized.forEach(({ i }) => autoSizingIds.delete(i));
        if (autoSizingIds.size === 0) {
          autoSizingRef.current = new Set();
        }
        return;
      }
      if (typeof window !== 'undefined') {
        console.info(
          '[tile-layout] onLayoutChange normalized',
          JSON.stringify(normalized.map(({ i, x, y, w, h }) => ({ i, x, y, w, h }))),
        );
      }
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
          const dims = clampGridSize(item.w, item.h, current, cols, true);
          const layoutChanged = !isSameLayoutDimensions(current.lastLayout, dims);
          if (!layoutChanged) {
            return;
          }
          if (!current.layoutInitialized) {
            nextState[item.i] = {
              ...current,
              lastLayout: dims,
            };
            changed = true;
            return;
          }
          nextState[item.i] = {
            ...current,
            lastLayout: dims,
            manualLayout: true,
            layoutHostCols: current.hostCols ?? current.layoutHostCols,
            layoutHostRows: current.hostRows ?? current.layoutHostRows,
          };
          changed = true;
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
    const state = tileStateRef.current[sessionId];
    const measurement = state?.measurements;
    const computeTarget = () => {
      const widthPx = Math.max(1, Math.round((measurement?.width ?? 0)));
      const heightPx = Math.max(1, Math.round((measurement?.height ?? 0)));
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
        control.trigger();
      }
    };
    // debounce to avoid bursts
    window.clearTimeout((scheduleHostResize as any)._t?.[sessionId]);
    (scheduleHostResize as any)._t = (scheduleHostResize as any)._t || {};
    (scheduleHostResize as any)._t[sessionId] = window.setTimeout(request, 180);
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
        const preview = next.preview ?? null;
        if (next.preview !== preview) {
          next = { ...next, preview };
        }

        const hostChanged =
          (current.hostCols ?? null) !== (next.hostCols ?? null) ||
          (current.hostRows ?? null) !== (next.hostRows ?? null);

        let computedZoom: number;
        if (next.locked) {
          computedZoom = MAX_UNLOCKED_ZOOM;
        } else if (preview) {
          const fallbackZoom = Number.isFinite(next.zoom)
            ? Number(next.zoom)
            : Number.isFinite(current.zoom)
              ? current.zoom
              : 1;
          computedZoom = fallbackZoom;
        } else {
          computedZoom = computeZoomForSize(
            next.measurements,
            next.hostCols,
            next.hostRows,
            next.viewportCols,
            next.viewportRows,
          );
        }

        if (next.locked) {
          next = { ...next, zoom: MAX_UNLOCKED_ZOOM };
        } else if (preview) {
          const baseMeasurement: TileMeasurements = {
            width: preview.targetWidth,
            height: preview.targetHeight,
          };
          let selected = Number.isFinite(next.zoom)
            ? Number(next.zoom)
            : Number.isFinite(current.zoom)
              ? current.zoom
              : 1;
          if (!Number.isFinite(selected)) {
            selected = 1;
          }
          if (!isSamePreview(current.preview, preview)) {
            const wasDefault = Math.abs(current.zoom - 1) < 0.05;
            if (wasDefault) {
              selected = 1;
            }
          }
          next = { ...next, zoom: clampZoom(selected, baseMeasurement) };
        } else {
          const measurementChanged = !isSameMeasurement(current.measurements, next.measurements);
          let selected = Number.isFinite(next.zoom) ? Number(next.zoom) : computedZoom;
          if (measurementChanged) {
            const previousTarget = computeZoomForSize(
              current.measurements,
              current.hostCols,
              current.hostRows,
              current.viewportCols,
              current.viewportRows,
            );
            const wasDefault =
              current.measurements === null ||
              Math.abs(current.zoom - previousTarget) < 0.05 ||
              Math.abs(current.zoom - DEFAULT_ZOOM) < 0.05;
            if (wasDefault) {
              selected = computedZoom;
            }
          } else if (hostChanged) {
            const previousTarget = computeZoomForSize(
              current.measurements,
              current.hostCols,
              current.hostRows,
              current.viewportCols,
              current.viewportRows,
            );
            const wasDefault =
              current.measurements === null ||
              !Number.isFinite(current.zoom) ||
              Math.abs(current.zoom - previousTarget) < 0.05 ||
              Math.abs(current.zoom - DEFAULT_ZOOM) < 0.05;
            if (wasDefault) {
              selected = computedZoom;
            }
          }
          next = { ...next, zoom: clampZoom(selected, next.measurements) };
        }

        if (preview) {
          const zoomFactor = next.locked ? MAX_UNLOCKED_ZOOM : next.zoom;
          const previewMeasurement: TileMeasurements = {
            width: preview.targetWidth * zoomFactor,
            height: preview.targetHeight * zoomFactor,
          };
          if (!isSameMeasurement(next.measurements, previewMeasurement)) {
            next = { ...next, measurements: previewMeasurement };
          }
          if (next.manualLayout) {
            next = { ...next, manualLayout: false };
          }
          if (next.layoutInitialized) {
            next = { ...next, layoutInitialized: false };
          }
        }

        if (typeof window !== 'undefined') {
          try {
            console.info('[tile-layout] state-derivation', {
              sessionId,
              previewActive: Boolean(preview),
              computedZoom,
              previousZoom: current.zoom,
              nextZoom: next.zoom,
              previousHostRows: current.hostRows,
              nextHostRows: next.hostRows,
              previousHostCols: current.hostCols,
              nextHostCols: next.hostCols,
              previousViewportRows: current.viewportRows,
              nextViewportRows: next.viewportRows,
              previousViewportCols: current.viewportCols,
              nextViewportCols: next.viewportCols,
              hasMeasurements: Boolean(next.measurements),
            });
          } catch {
            // ignore logging errors
          }
        }

        if (isTileStateEqual(current, next)) {
          return prev;
        }
        return { ...prev, [sessionId]: next };
      });
    },
    [],
  );

  const handleTilePreviewMeasurementsChange = useCallback(
    (sessionId: string, measurement: PreviewMetrics | null) => {
      updateTileState(sessionId, (state) => {
        if (!measurement) {
          return {
            ...state,
            preview: null,
            measurements: state.manualLayout ? state.measurements : null,
          };
        }
        // Ignore stale measurements from an earlier version
        const prevVersion = state.preview?.measurementVersion ?? 0;
        if (measurement.measurementVersion < prevVersion) {
          if (typeof window !== 'undefined') {
            try {
              console.info('[tile-layout] preview-skip-stale', {
                sessionId,
                prevVersion,
                nextVersion: measurement.measurementVersion,
              });
            } catch {}
          }
          return state;
        }
        const zoomFactor = state.locked ? MAX_UNLOCKED_ZOOM : state.zoom;
        return {
          ...state,
          preview: measurement,
          measurements: {
            width: measurement.targetWidth * zoomFactor,
            height: measurement.targetHeight * zoomFactor,
          },
          hasHostDimensions: true,
        };
      });
    },
    [updateTileState],
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
      const state = tileStateRef.current[newItem.i] ?? buildTileState();
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
      updateTileState(newItem.i, (current) => ({
        ...current,
        measurements: (() => {
          if (normalized.length === 0) return current.measurements;
          const layoutEntry = normalized.find((item) => item.i === newItem.i);
          if (!layoutEntry) return current.measurements;
      const columnWidth = getColumnWidth(gridWidth, cols);
      const fallbackColumnWidth =
        columnWidth != null
          ? columnWidth
          : newItem.w > 0 && widthPx > 0
            ? widthPx / newItem.w
            : null;
      const widthEstimate =
        fallbackColumnWidth != null
          ? Math.round(fallbackColumnWidth * layoutEntry.w * 1000) / 1000
          : widthPx;
          const heightEstimate = Math.max(ROW_HEIGHT * layoutEntry.h, heightPx);
          if (!Number.isFinite(widthEstimate) || !Number.isFinite(heightEstimate) || widthEstimate <= 0 || heightEstimate <= 0) {
            return current.measurements;
          }
          if (typeof window !== 'undefined') {
            try {
              console.info('[tile-diag] manual-measure', {
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
          return {
            width: widthEstimate,
            height: heightEstimate,
          };
        })(),
      }));
      if (state.locked) {
        scheduleHostResize(newItem.i);
      }
    },
    [clampLayoutItems, cols, gridWidth, handleLayoutChange, handleLayoutCommit, scheduleHostResize, updateTileState],
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
      const state = tileStateRef.current[sessionId];
      if (!state || !state.manualLayout || autoSizingRef.current.has(sessionId)) {
        return;
      }
      const columnWidth = getColumnWidth(gridWidth, cols);
      if (columnWidth == null) {
        return;
      }
      const layoutWidth =
        columnWidth * layoutItem.w;
      const layoutHeight = Math.max(ROW_HEIGHT * layoutItem.h, 1);
      if (!Number.isFinite(layoutWidth) || layoutWidth <= 0) {
        return;
      }
      const normalized: TileMeasurements = {
        width: layoutWidth,
        height: layoutHeight,
      };
      const existing = tileStateRef.current[sessionId]?.measurements;
      if (existing && isSameMeasurement(existing, normalized)) {
        return;
      }
      updateTileState(sessionId, (state) => ({
        ...state,
        measurements: normalized,
      }));
      if (typeof window !== 'undefined') {
        console.info(
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
      }
    },
    [cols, gridWidth, layoutMap, updateTileState],
  );

  const handlePreviewStatusChange = (
    sessionId: string,
    status: 'connecting' | 'initializing' | 'ready' | 'error',
  ) => {
    updateTileState(sessionId, (state) => {
      if (state.previewStatus === status) {
        return state;
      }
      return {
        ...state,
        previewStatus: status,
      };
    });
  };

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
      if (typeof window !== 'undefined') {
        try {
          console.info('[tile-layout] viewport-payload', {
            version: 'v2',
            sessionId,
            viewportRows: dims?.viewportRows ?? null,
            viewportCols: dims?.viewportCols ?? null,
            hostRows: dims?.hostRows ?? null,
            hostCols: dims?.hostCols ?? null,
          });
        } catch {
          // ignore logging errors
        }
      }
      if (
        !dims ||
        typeof dims.viewportRows !== 'number' ||
        typeof dims.viewportCols !== 'number' ||
        dims.viewportRows <= 0 ||
        dims.viewportCols <= 0
      ) {
        if (typeof window !== 'undefined') {
          console.warn('[tile-layout] viewport-dims skipped', { sessionId, dims });
        }
        return;
      }
      if (typeof window !== 'undefined') {
        console.info('[tile-layout] viewport-dims raw', JSON.stringify(dims));
        console.info('[tile-layout] viewport-dims', {
          version: 'v1',
          sessionId,
          viewportRows: dims.viewportRows,
          viewportCols: dims.viewportCols,
          hostRows: dims.hostRows,
          hostCols: dims.hostCols,
        });
      }
      updateTileState(sessionId, (state) => {
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
        if (typeof window !== 'undefined') {
          try {
            console.info('[tile-layout] viewport-apply', {
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
          } catch {
            // ignore logging issues
          }
        }
        return nextState;
      });
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
      const unitHeight = ROW_HEIGHT;
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
      updateTileState(sessionId, (current) => ({
        ...current,
        locked: false,
        zoom: clampZoom(spansFullWidth ? MAX_UNLOCKED_ZOOM : DEFAULT_ZOOM),
        manualLayout: false,
        layoutHostCols: state.hostCols ?? current.layoutHostCols,
        layoutHostRows: state.hostRows ?? current.layoutHostRows,
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
      {typeof window !== 'undefined' &&
        console.info('[tile-layout] layout-signature', layoutSignature)}
      <AutoGrid
        layout={layoutForRender}
        cols={cols}
        rowHeight={ROW_HEIGHT}
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
          const view = tileState[session.session_id] ?? buildTileState();
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
              view={view}
              onMeasure={(measurement) => handleMeasure(session.session_id, measurement)}
              onViewport={handleViewportDimensions}
              onPreviewStatusChange={handlePreviewStatusChange}
              onPreviewMeasurementsChange={handleTilePreviewMeasurementsChange}
              onHostResizeStateChange={handleHostResizeStateChange}
              onViewerStateChange={handleViewerStateChange}
              isExpanded={isExpanded}
              className="session-grid-item"
              viewerOverride={effectiveViewerOverrides[session.session_id] ?? null}
            />
          );
        })}
      </AutoGrid>
    </div>
  ) : (
    <div className="h-[520px] rounded-xl border border-border bg-card shadow-sm" />
  );

  useEffect(() => {
    if (typeof window === 'undefined') return;
    console.info('[tile-layout] instrumentation', { component: 'TileCanvas', version: 'v1' });
  }, []);

  useEffect(() => {
    if (!isClient) return;
    const wrapper = gridWrapperRef.current;
    if (!wrapper) return;
    const target =
      gridElementNode ??
      wrapper.querySelector<HTMLElement>('.react-grid-layout') ??
      wrapper.parentElement?.querySelector<HTMLElement>('.react-grid-layout') ??
      wrapper;
    if (!target) {
      console.info('[tile-layout] dom-log skipped', { version: 'v1', layoutSignature });
      return;
    }
    console.info('[tile-layout] dom-log start', {
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
        console.info(
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
        console.info(
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
            console.info('[tile-layout] dom-mutation', {
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

  const expandedViewer = expanded ? viewerStates[expanded.session_id] ?? null : null;

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
            {expandedViewer ? (
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
                onViewportDimensions={(sessionIdArg, dims) => {
                  if (!dims) {
                    return;
                  }
                  handleViewportDimensions(sessionIdArg ?? expanded.session_id, dims);
                }}
                onPreviewMeasurementsChange={(sessionIdArg, measurement) => {
                  handleTilePreviewMeasurementsChange(
                    sessionIdArg ?? expanded.session_id,
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
                viewerOverride={expandedViewer}
              />
            ) : (
              <div className="flex h-full items-center justify-center bg-neutral-950 text-sm text-muted-foreground">
                <span>Preparing viewer…</span>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
