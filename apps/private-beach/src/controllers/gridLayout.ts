'use client';

import type { Layout as ReactGridLayoutItem } from 'react-grid-layout';
import type { BeachLayoutItem } from '../lib/api';
import type { CanvasLayout as SharedCanvasLayout, CanvasTileNode } from '../canvas';
import {
  defaultTileViewState,
  mergeTileViewState,
  type PreviewMetrics,
  type TileMeasurements,
  type TileViewState,
} from './gridViewState';

export const GRID_LAYOUT_VERSION = 2;
export const DEFAULT_GRID_COLS = 128;
export const DEFAULT_ROW_HEIGHT_PX = 12;
export const DEFAULT_GRID_W_UNITS = 32;
export const DEFAULT_GRID_H_UNITS = 28;

export type GridLayoutUnits = {
  x: number;
  y: number;
  w: number;
  h: number;
};

export type GridDashboardMetadata = {
  layout: GridLayoutUnits;
  gridCols?: number;
  rowHeightPx?: number;
  layoutVersion?: number;
  widthPx?: number;
  heightPx?: number;
  zoom?: number;
  locked?: boolean;
  toolbarPinned?: boolean;
  manualLayout?: boolean;
  hostCols?: number | null;
  hostRows?: number | null;
  measurementVersion?: number;
  measurementSource?: string | null;
  measurements?: TileMeasurements | null;
  viewportCols?: number | null;
  viewportRows?: number | null;
  layoutInitialized?: boolean;
  layoutHostCols?: number | null;
  layoutHostRows?: number | null;
  hasHostDimensions?: boolean;
  preview?: GridPreviewMetrics | null;
  previewStatus?: 'connecting' | 'initializing' | 'ready' | 'error';
  viewState?: TileViewState;
};

export type TileGridMetadataUpdate = Partial<Omit<GridDashboardMetadata, 'layout' | 'viewState'>> & {
  layout?: Partial<GridLayoutUnits>;
  viewState?: Partial<TileViewState>;
};

export type GridLayoutSnapshot = {
  tiles: Record<string, GridDashboardMetadata>;
  gridCols?: number;
  rowHeightPx?: number;
  layoutVersion?: number;
};

type DashboardMetadataRecord = Record<string, unknown> & {
  layout?: Record<string, unknown>;
};

const DASHBOARD_KEY = 'dashboard';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function optionalNumber(value: unknown): number | undefined {
  if (isFiniteNumber(value)) {
    return value;
  }
  return undefined;
}

function optionalNullableNumber(value: unknown): number | null | undefined {
  if (value === null) {
    return null;
  }
  if (isFiniteNumber(value)) {
    return value;
  }
  return undefined;
}

function optionalBoolean(value: unknown): boolean | undefined {
  if (typeof value === 'boolean') {
    return value;
  }
  return undefined;
}

function optionalPreviewStatus(value: unknown): GridDashboardMetadata['previewStatus'] | undefined {
  if (value === 'connecting' || value === 'initializing' || value === 'ready' || value === 'error') {
    return value;
  }
  return undefined;
}

function optionalMeasurements(value: unknown): TileMeasurements | null | undefined {
  if (value === null) {
    return null;
  }
  if (!isRecord(value)) {
    return undefined;
  }
  const width = optionalNumber(value.width);
  const height = optionalNumber(value.height);
  if (typeof width === 'number' && typeof height === 'number') {
    return { width, height };
  }
  return undefined;
}

function optionalPreview(value: unknown): PreviewMetrics | null | undefined {
  if (value === null) {
    return null;
  }
  if (!isRecord(value)) {
    return undefined;
  }
  const scale = optionalNumber(value.scale);
  const targetWidth = optionalNumber(value.targetWidth);
  const targetHeight = optionalNumber(value.targetHeight);
  const rawWidth = optionalNumber(value.rawWidth);
  const rawHeight = optionalNumber(value.rawHeight);
  const hostRows = optionalNullableNumber(value.hostRows);
  const hostCols = optionalNullableNumber(value.hostCols);
  const measurementVersion = optionalNumber(value.measurementVersion);
  if (
    typeof scale === 'number' &&
    typeof targetWidth === 'number' &&
    typeof targetHeight === 'number' &&
    typeof rawWidth === 'number' &&
    typeof rawHeight === 'number' &&
    typeof measurementVersion === 'number'
  ) {
    return {
      scale,
      targetWidth,
      targetHeight,
      rawWidth,
      rawHeight,
      hostRows: hostRows ?? null,
      hostCols: hostCols ?? null,
      measurementVersion,
    };
  }
  return undefined;
}

function normalizeViewState(value: unknown, base?: TileViewState): TileViewState {
  const state: TileViewState = base ? { ...base } : defaultTileViewState();
  if (!isRecord(value)) {
    return state;
  }

  const zoom = optionalNumber(value.zoom);
  if (typeof zoom === 'number') {
    state.zoom = zoom;
  }

  if (typeof value.locked === 'boolean') {
    state.locked = value.locked;
  }

  if (typeof value.toolbarPinned === 'boolean') {
    state.toolbarPinned = value.toolbarPinned;
  }

  const measurements = optionalMeasurements(value.measurements);
  if (measurements !== undefined) {
    state.measurements = measurements ?? null;
  }

  const hostCols = optionalNullableNumber(value.hostCols);
  if (hostCols !== undefined) {
    state.hostCols = hostCols;
  }

  const hostRows = optionalNullableNumber(value.hostRows);
  if (hostRows !== undefined) {
    state.hostRows = hostRows;
  }

  if (typeof value.hasHostDimensions === 'boolean') {
    state.hasHostDimensions = value.hasHostDimensions;
  }

  const viewportCols = optionalNullableNumber(value.viewportCols);
  if (viewportCols !== undefined) {
    state.viewportCols = viewportCols;
  }

  const viewportRows = optionalNullableNumber(value.viewportRows);
  if (viewportRows !== undefined) {
    state.viewportRows = viewportRows;
  }

  const lastLayoutRaw = value.lastLayout;
  if (lastLayoutRaw === null) {
    state.lastLayout = null;
  } else if (isRecord(lastLayoutRaw)) {
    const w = optionalNumber(lastLayoutRaw.w);
    const h = optionalNumber(lastLayoutRaw.h);
    if (typeof w === 'number' && typeof h === 'number') {
      state.lastLayout = { w, h };
    }
  }

  if (typeof value.layoutInitialized === 'boolean') {
    state.layoutInitialized = value.layoutInitialized;
  }

  if (typeof value.manualLayout === 'boolean') {
    state.manualLayout = value.manualLayout;
  }

  const layoutHostCols = optionalNullableNumber(value.layoutHostCols);
  if (layoutHostCols !== undefined) {
    state.layoutHostCols = layoutHostCols;
  }

  const layoutHostRows = optionalNullableNumber(value.layoutHostRows);
  if (layoutHostRows !== undefined) {
    state.layoutHostRows = layoutHostRows;
  }

  const previewStatus = optionalPreviewStatus(value.previewStatus);
  if (previewStatus !== undefined) {
    state.previewStatus = previewStatus;
  }

  const preview = optionalPreview(value.preview);
  if (preview !== undefined) {
    state.preview = preview ?? null;
  }

  return state;
}

function viewStateEquals(a: TileViewState | undefined, b: TileViewState | undefined): boolean {
  const av = a ?? defaultTileViewState();
  const bv = b ?? defaultTileViewState();
  return (
    av.zoom === bv.zoom &&
    av.locked === bv.locked &&
    av.toolbarPinned === bv.toolbarPinned &&
    measurementsEqual(av.measurements, bv.measurements) &&
    av.hostCols === bv.hostCols &&
    av.hostRows === bv.hostRows &&
    av.hasHostDimensions === bv.hasHostDimensions &&
    av.viewportCols === bv.viewportCols &&
    av.viewportRows === bv.viewportRows &&
    layoutEqual(av.lastLayout, bv.lastLayout) &&
    av.layoutInitialized === bv.layoutInitialized &&
    av.manualLayout === bv.manualLayout &&
    av.layoutHostCols === bv.layoutHostCols &&
    av.layoutHostRows === bv.layoutHostRows &&
    av.previewStatus === bv.previewStatus &&
    previewEqual(av.preview, bv.preview)
  );
}

function measurementsEqual(a: TileMeasurements | null, b: TileMeasurements | null): boolean {
  if (!a && !b) return true;
  if (!a || !b) return false;
  return Math.abs(a.width - b.width) < 0.0001 && Math.abs(a.height - b.height) < 0.0001;
}

function previewEqual(a: PreviewMetrics | null, b: PreviewMetrics | null): boolean {
  if (!a && !b) return true;
  if (!a || !b) return false;
  return (
    Math.abs(a.scale - b.scale) < 0.0001 &&
    Math.abs(a.targetWidth - b.targetWidth) < 0.0001 &&
    Math.abs(a.targetHeight - b.targetHeight) < 0.0001 &&
    Math.abs(a.rawWidth - b.rawWidth) < 0.0001 &&
    Math.abs(a.rawHeight - b.rawHeight) < 0.0001 &&
    a.hostRows === b.hostRows &&
    a.hostCols === b.hostCols &&
    a.measurementVersion === b.measurementVersion
  );
}

function layoutEqual(
  a: Partial<GridLayoutUnits> | null,
  b: Partial<GridLayoutUnits> | null,
): boolean {
  if (!a && !b) return true;
  if (!a || !b) return false;
  return (
    (a.x ?? 0) === (b.x ?? 0) &&
    (a.y ?? 0) === (b.y ?? 0) &&
    (a.w ?? 0) === (b.w ?? 0) &&
    (a.h ?? 0) === (b.h ?? 0)
  );
}

function clampInt(value: number, min: number, max?: number): number {
  const clampedMin = Math.max(min, Math.floor(value));
  if (typeof max === 'number' && Number.isFinite(max)) {
    return Math.min(Math.floor(max), clampedMin);
  }
  return clampedMin;
}

function sanitizeLayoutUnits(input: Partial<GridLayoutUnits> | undefined, defaults: GridLayoutUnits): GridLayoutUnits {
  if (!input) return defaults;
  const x = isFiniteNumber(input.x) ? clampInt(input.x, 0) : defaults.x;
  const y = isFiniteNumber(input.y) ? clampInt(input.y, 0) : defaults.y;
  const w = isFiniteNumber(input.w) ? clampInt(input.w, 1) : defaults.w;
  const h = isFiniteNumber(input.h) ? clampInt(input.h, 1) : defaults.h;
  return { x, y, w, h };
}

function layoutDefaults(): GridLayoutUnits {
  return { x: 0, y: 0, w: DEFAULT_GRID_W_UNITS, h: DEFAULT_GRID_H_UNITS };
}

function cloneTile(tile: CanvasTileNode, metadata: Record<string, unknown>): CanvasTileNode {
  return {
    ...tile,
    metadata,
  };
}

export function extractGridDashboardMetadata(tile: CanvasTileNode | null | undefined): GridDashboardMetadata {
  const defaults = layoutDefaults();
  const layout = tile?.position
    ? ({
        x: clampInt(tile.position.x, 0),
        y: clampInt(tile.position.y, 0),
        w: DEFAULT_GRID_W_UNITS,
        h: DEFAULT_GRID_H_UNITS,
      } satisfies GridLayoutUnits)
    : defaults;
  const metadata = (tile?.metadata ?? {}) as DashboardMetadataRecord;
  const dashboardRaw = (isRecord(metadata[DASHBOARD_KEY]) ? metadata[DASHBOARD_KEY] : {}) as DashboardMetadataRecord;
  const layoutRaw = isRecord(dashboardRaw.layout) ? (dashboardRaw.layout as Record<string, unknown>) : {};
  const measurements = optionalMeasurements(dashboardRaw.measurements) ?? optionalMeasurements(metadata.measurements) ?? null;
  const preview = optionalPreview(dashboardRaw.preview) ?? optionalPreview(metadata.preview) ?? null;
  const computedLayout = sanitizeLayoutUnits(
    {
      x: optionalNumber(layoutRaw.x),
      y: optionalNumber(layoutRaw.y),
      w: optionalNumber(layoutRaw.w),
      h: optionalNumber(layoutRaw.h),
    },
    {
      x: layout.x,
      y: layout.y,
      w: DEFAULT_GRID_W_UNITS,
      h: DEFAULT_GRID_H_UNITS,
    },
  );
  const measurementVersion =
    optionalNumber(dashboardRaw.measurementVersion) ?? optionalNumber(metadata.measurementVersion);
  const measurementSource =
    typeof dashboardRaw.measurementSource === 'string'
      ? dashboardRaw.measurementSource
      : typeof metadata.measurementSource === 'string'
        ? (metadata.measurementSource as string)
        : null;
  const zoom = optionalNumber(dashboardRaw.zoom) ?? optionalNumber(metadata.zoom);
  const locked = optionalBoolean(dashboardRaw.locked) ?? optionalBoolean(metadata.locked);
  const toolbarPinned = optionalBoolean(dashboardRaw.toolbarPinned) ?? optionalBoolean(metadata.toolbarPinned);
  const manualLayout = optionalBoolean(dashboardRaw.manualLayout) ?? optionalBoolean(metadata.manualLayout);
  const hostCols =
    optionalNullableNumber(dashboardRaw.hostCols) ??
    optionalNullableNumber(metadata.hostCols) ??
    optionalNullableNumber(dashboardRaw.hostColumns);
  const hostRows =
    optionalNullableNumber(dashboardRaw.hostRows) ??
    optionalNullableNumber(metadata.hostRows) ??
    optionalNullableNumber(dashboardRaw.hostRows);
  const viewportCols = optionalNullableNumber(dashboardRaw.viewportCols) ?? optionalNullableNumber(metadata.viewportCols);
  const viewportRows = optionalNullableNumber(dashboardRaw.viewportRows) ?? optionalNullableNumber(metadata.viewportRows);
  const layoutInitialized =
    typeof dashboardRaw.layoutInitialized === 'boolean'
      ? dashboardRaw.layoutInitialized
      : typeof metadata.layoutInitialized === 'boolean'
        ? (metadata.layoutInitialized as boolean)
        : undefined;
  const layoutHostCols =
    optionalNullableNumber(dashboardRaw.layoutHostCols) ?? optionalNullableNumber(metadata.layoutHostCols);
  const layoutHostRows =
    optionalNullableNumber(dashboardRaw.layoutHostRows) ?? optionalNullableNumber(metadata.layoutHostRows);
  const hasHostDimensions =
    typeof dashboardRaw.hasHostDimensions === 'boolean'
      ? dashboardRaw.hasHostDimensions
      : typeof metadata.hasHostDimensions === 'boolean'
        ? (metadata.hasHostDimensions as boolean)
        : undefined;
  const previewStatus = optionalPreviewStatus(dashboardRaw.previewStatus) ?? optionalPreviewStatus(metadata.previewStatus);

  const viewStateBaseline: Partial<TileViewState> = {
    measurements,
    preview,
  };
  if (typeof zoom === 'number') {
    viewStateBaseline.zoom = zoom;
  }
  if (typeof locked === 'boolean') {
    viewStateBaseline.locked = locked;
  }
  if (typeof toolbarPinned === 'boolean') {
    viewStateBaseline.toolbarPinned = toolbarPinned;
  }
  if (typeof manualLayout === 'boolean') {
    viewStateBaseline.manualLayout = manualLayout;
  }
  if (hostCols !== undefined) {
    viewStateBaseline.hostCols = hostCols;
  }
  if (hostRows !== undefined) {
    viewStateBaseline.hostRows = hostRows;
  }
  if (typeof hasHostDimensions === 'boolean') {
    viewStateBaseline.hasHostDimensions = hasHostDimensions;
  }
  if (viewportCols !== undefined) {
    viewStateBaseline.viewportCols = viewportCols;
  }
  if (viewportRows !== undefined) {
    viewStateBaseline.viewportRows = viewportRows;
  }
  if (layoutInitialized !== undefined) {
    viewStateBaseline.layoutInitialized = layoutInitialized;
  }
  if (layoutHostCols !== undefined) {
    viewStateBaseline.layoutHostCols = layoutHostCols;
  }
  if (layoutHostRows !== undefined) {
    viewStateBaseline.layoutHostRows = layoutHostRows;
  }
  if (previewStatus !== undefined) {
    viewStateBaseline.previewStatus = previewStatus;
  }

  const baseViewState = mergeTileViewState(defaultTileViewState(), viewStateBaseline);
  const viewState = normalizeViewState(dashboardRaw.viewState ?? metadata.viewState, baseViewState);
  return {
    layout: computedLayout,
    gridCols:
      optionalNumber(dashboardRaw.gridCols) ??
      optionalNumber(metadata.gridCols) ??
      optionalNumber(dashboardRaw.cols) ??
      DEFAULT_GRID_COLS,
    rowHeightPx:
      optionalNumber(dashboardRaw.rowHeightPx) ??
      optionalNumber(metadata.rowHeightPx) ??
      optionalNumber(dashboardRaw.rowHeight) ??
      DEFAULT_ROW_HEIGHT_PX,
    layoutVersion: optionalNumber(dashboardRaw.layoutVersion) ?? optionalNumber(metadata.layoutVersion),
    widthPx: optionalNumber(dashboardRaw.widthPx) ?? optionalNumber(metadata.widthPx) ?? optionalNumber(tile?.size?.width),
    heightPx:
      optionalNumber(dashboardRaw.heightPx) ?? optionalNumber(metadata.heightPx) ?? optionalNumber(tile?.size?.height),
    zoom: viewState.zoom,
    locked: viewState.locked,
    toolbarPinned: viewState.toolbarPinned,
    manualLayout: viewState.manualLayout,
    hostCols: viewState.hostCols,
    hostRows: viewState.hostRows,
    measurementVersion,
    measurementSource,
    measurements: viewState.measurements,
    viewportCols: viewState.viewportCols,
    viewportRows: viewState.viewportRows,
    layoutInitialized: viewState.layoutInitialized,
    layoutHostCols: viewState.layoutHostCols,
    layoutHostRows: viewState.layoutHostRows,
    hasHostDimensions: viewState.hasHostDimensions,
    preview: viewState.preview,
    previewStatus: viewState.previewStatus,
    viewState,
  };
}

function metadataEquals(a: GridDashboardMetadata, b: GridDashboardMetadata): boolean {
  return (
    a.layout.x === b.layout.x &&
    a.layout.y === b.layout.y &&
    a.layout.w === b.layout.w &&
    a.layout.h === b.layout.h &&
    a.gridCols === b.gridCols &&
    a.rowHeightPx === b.rowHeightPx &&
    a.layoutVersion === b.layoutVersion &&
    a.widthPx === b.widthPx &&
    a.heightPx === b.heightPx &&
    a.zoom === b.zoom &&
    a.locked === b.locked &&
    a.toolbarPinned === b.toolbarPinned &&
    a.manualLayout === b.manualLayout &&
    a.hostCols === b.hostCols &&
    a.hostRows === b.hostRows &&
    a.measurementVersion === b.measurementVersion &&
    a.measurementSource === b.measurementSource &&
    measurementsEqual(a.measurements ?? null, b.measurements ?? null) &&
    a.viewportCols === b.viewportCols &&
    a.viewportRows === b.viewportRows &&
    (a.layoutInitialized ?? false) === (b.layoutInitialized ?? false) &&
    a.layoutHostCols === b.layoutHostCols &&
    a.layoutHostRows === b.layoutHostRows &&
    (a.hasHostDimensions ?? false) === (b.hasHostDimensions ?? false) &&
    previewEqual(a.preview ?? null, b.preview ?? null) &&
    a.previewStatus === b.previewStatus &&
    viewStateEquals(a.viewState, b.viewState)
  );
}

export function withTileGridMetadata(tile: CanvasTileNode, update: TileGridMetadataUpdate): CanvasTileNode {
  const current = extractGridDashboardMetadata(tile);
  const nextLayout = sanitizeLayoutUnits(update.layout, current.layout);
  const has = (key: keyof TileGridMetadataUpdate) => Object.prototype.hasOwnProperty.call(update, key);
  const baseViewState = current.viewState ?? defaultTileViewState();
  let nextViewState = baseViewState;
  let viewStateChanged = false;

  const mergeIntoViewState = (partial: Partial<TileViewState> | undefined) => {
    if (!partial) {
      return;
    }
    const merged = mergeTileViewState(nextViewState, partial);
    if (viewStateEquals(merged, nextViewState)) {
      return;
    }
    nextViewState = merged;
    viewStateChanged = true;
  };

  if (has('viewState')) {
    mergeIntoViewState(update.viewState);
  }
  if (has('zoom') && typeof update.zoom === 'number') {
    mergeIntoViewState({ zoom: update.zoom });
  }
  if (has('locked') && typeof update.locked === 'boolean') {
    mergeIntoViewState({ locked: update.locked });
  }
  if (has('toolbarPinned') && typeof update.toolbarPinned === 'boolean') {
    mergeIntoViewState({ toolbarPinned: update.toolbarPinned });
  }
  if (has('manualLayout') && typeof update.manualLayout === 'boolean') {
    mergeIntoViewState({ manualLayout: update.manualLayout });
  }
  if (has('hostCols')) {
    mergeIntoViewState({ hostCols: update.hostCols ?? null });
  }
  if (has('hostRows')) {
    mergeIntoViewState({ hostRows: update.hostRows ?? null });
  }
  if (has('hasHostDimensions') && typeof update.hasHostDimensions === 'boolean') {
    mergeIntoViewState({ hasHostDimensions: update.hasHostDimensions });
  }
  if (has('viewportCols')) {
    mergeIntoViewState({ viewportCols: update.viewportCols ?? null });
  }
  if (has('viewportRows')) {
    mergeIntoViewState({ viewportRows: update.viewportRows ?? null });
  }
  if (has('layoutInitialized') && typeof update.layoutInitialized === 'boolean') {
    mergeIntoViewState({ layoutInitialized: update.layoutInitialized });
  }
  if (has('layoutHostCols')) {
    mergeIntoViewState({ layoutHostCols: update.layoutHostCols ?? null });
  }
  if (has('layoutHostRows')) {
    mergeIntoViewState({ layoutHostRows: update.layoutHostRows ?? null });
  }
  if (has('measurements')) {
    mergeIntoViewState({ measurements: update.measurements ?? null });
  }
  if (has('preview')) {
    mergeIntoViewState({ preview: update.preview ?? null });
  }
  if (has('previewStatus') && typeof update.previewStatus === 'string') {
    mergeIntoViewState({ previewStatus: update.previewStatus as TileViewState['previewStatus'] });
  }

  const fallbackCols = update.gridCols ?? current.gridCols ?? DEFAULT_GRID_W_UNITS;
  const maxX = Math.max(0, fallbackCols - nextLayout.w);
  const layoutOverrideX =
    typeof update.layout?.x === 'number' ? clampInt(update.layout.x, 0, maxX) : nextLayout.x;
  const layoutOverrideY = typeof update.layout?.y === 'number' ? clampInt(update.layout.y, 0) : nextLayout.y;
  const layoutWithOverride: GridLayoutUnits = {
    ...nextLayout,
    x: layoutOverrideX,
    y: layoutOverrideY,
  };
  const merged: GridDashboardMetadata = {
    layout: layoutWithOverride,
    gridCols: has('gridCols') ? update.gridCols ?? current.gridCols : current.gridCols,
    rowHeightPx: has('rowHeightPx') ? update.rowHeightPx ?? current.rowHeightPx : current.rowHeightPx,
    layoutVersion: has('layoutVersion') ? update.layoutVersion ?? current.layoutVersion : current.layoutVersion,
    widthPx: has('widthPx') ? update.widthPx ?? current.widthPx : current.widthPx,
    heightPx: has('heightPx') ? update.heightPx ?? current.heightPx : current.heightPx,
    zoom:
      has('zoom') && typeof update.zoom === 'number'
        ? update.zoom
        : viewStateChanged
          ? nextViewState.zoom
          : current.zoom,
    locked:
      has('locked') && typeof update.locked === 'boolean'
        ? update.locked
        : viewStateChanged
          ? nextViewState.locked
          : current.locked,
    toolbarPinned:
      has('toolbarPinned') && typeof update.toolbarPinned === 'boolean'
        ? update.toolbarPinned
        : viewStateChanged
          ? nextViewState.toolbarPinned
          : current.toolbarPinned,
    manualLayout:
      has('manualLayout') && typeof update.manualLayout === 'boolean'
        ? update.manualLayout
        : viewStateChanged
          ? nextViewState.manualLayout
          : current.manualLayout,
    hostCols:
      has('hostCols')
        ? update.hostCols ?? null
        : viewStateChanged
          ? nextViewState.hostCols
          : current.hostCols,
    hostRows:
      has('hostRows')
        ? update.hostRows ?? null
        : viewStateChanged
          ? nextViewState.hostRows
          : current.hostRows,
    measurementVersion: has('measurementVersion') ? update.measurementVersion : current.measurementVersion,
    measurementSource: has('measurementSource') ? update.measurementSource ?? null : current.measurementSource,
    measurements:
      has('measurements')
        ? update.measurements ?? null
        : viewStateChanged
          ? nextViewState.measurements
          : current.measurements,
    viewportCols:
      has('viewportCols')
        ? update.viewportCols ?? null
        : viewStateChanged
          ? nextViewState.viewportCols
          : current.viewportCols,
    viewportRows:
      has('viewportRows')
        ? update.viewportRows ?? null
        : viewStateChanged
          ? nextViewState.viewportRows
          : current.viewportRows,
    layoutInitialized:
      has('layoutInitialized')
        ? update.layoutInitialized ?? current.layoutInitialized
        : viewStateChanged
          ? nextViewState.layoutInitialized
          : current.layoutInitialized,
    layoutHostCols:
      has('layoutHostCols')
        ? update.layoutHostCols ?? null
        : viewStateChanged
          ? nextViewState.layoutHostCols
          : current.layoutHostCols,
    layoutHostRows:
      has('layoutHostRows')
        ? update.layoutHostRows ?? null
        : viewStateChanged
          ? nextViewState.layoutHostRows
          : current.layoutHostRows,
    hasHostDimensions:
      has('hasHostDimensions')
        ? update.hasHostDimensions ?? current.hasHostDimensions
        : viewStateChanged
          ? nextViewState.hasHostDimensions
          : current.hasHostDimensions,
    preview:
      has('preview')
        ? update.preview ?? null
        : viewStateChanged
          ? nextViewState.preview
          : current.preview,
    previewStatus:
      has('previewStatus')
        ? update.previewStatus ?? current.previewStatus
        : viewStateChanged
          ? nextViewState.previewStatus
          : current.previewStatus,
    viewState: viewStateChanged ? nextViewState : current.viewState,
  };
  const positionChanged =
    tile.position?.x !== merged.layout.x || tile.position?.y !== merged.layout.y;
  if (!positionChanged && metadataEquals(current, merged)) {
    return tile;
  }
  const metadata = { ...(tile.metadata ?? {}) };
  metadata[DASHBOARD_KEY] = {
    layout: merged.layout,
    gridCols: merged.gridCols,
    rowHeightPx: merged.rowHeightPx,
    layoutVersion: merged.layoutVersion,
    widthPx: merged.widthPx,
    heightPx: merged.heightPx,
    zoom: merged.zoom,
    locked: merged.locked,
    toolbarPinned: merged.toolbarPinned,
    manualLayout: merged.manualLayout,
    hostCols: merged.hostCols,
    hostRows: merged.hostRows,
    measurementVersion: merged.measurementVersion,
    measurementSource: merged.measurementSource ?? null,
    measurements: merged.measurements ?? null,
    viewportCols: merged.viewportCols ?? null,
    viewportRows: merged.viewportRows ?? null,
    layoutInitialized: merged.layoutInitialized ?? false,
    layoutHostCols: merged.layoutHostCols ?? null,
    layoutHostRows: merged.layoutHostRows ?? null,
    hasHostDimensions: merged.hasHostDimensions ?? false,
    preview: merged.preview ?? null,
    previewStatus: merged.previewStatus ?? 'connecting',
    viewState: merged.viewState ?? defaultTileViewState(),
  };
  return {
    ...cloneTile(tile, metadata),
    position: {
      x: merged.layout.x,
      y: merged.layout.y,
    },
  };
}

export function applyGridMetadataToLayout(
  layout: SharedCanvasLayout,
  updates: Record<string, TileGridMetadataUpdate>,
): SharedCanvasLayout {
  const nextTiles: SharedCanvasLayout['tiles'] = {};
  let didMutate = false;
  for (const [tileId, tile] of Object.entries(layout.tiles)) {
    const update = updates[tileId];
    if (!update) {
      nextTiles[tileId] = tile;
      continue;
    }
    const nextTile = withTileGridMetadata(tile, update);
    nextTiles[tileId] = nextTile;
    if (nextTile !== tile) {
      didMutate = true;
    }
  }
  if (!didMutate) {
    return layout;
  }
  return {
    ...layout,
    tiles: {
      ...layout.tiles,
      ...nextTiles,
    },
  };
}

export function extractGridLayoutSnapshot(layout: SharedCanvasLayout): GridLayoutSnapshot {
  const tiles: GridLayoutSnapshot['tiles'] = {};
  for (const [tileId, tile] of Object.entries(layout.tiles)) {
    tiles[tileId] = extractGridDashboardMetadata(tile);
  }
  const layoutMetadata = isRecord(layout.metadata?.dashboard) ? (layout.metadata.dashboard as Record<string, unknown>) : {};
  return {
    tiles,
    gridCols: optionalNumber(layoutMetadata.gridCols),
    rowHeightPx: optionalNumber(layoutMetadata.rowHeightPx),
    layoutVersion: optionalNumber(layoutMetadata.layoutVersion),
  };
}

export function beachItemsToGridSnapshot(
  items: BeachLayoutItem[],
  options?: { defaultCols?: number; defaultRowHeightPx?: number; layoutVersion?: number },
): GridLayoutSnapshot {
  const tiles: GridLayoutSnapshot['tiles'] = {};
  const defaultCols = options?.defaultCols ?? DEFAULT_GRID_COLS;
  const defaultRowHeight = options?.defaultRowHeightPx ?? DEFAULT_ROW_HEIGHT_PX;
  items.forEach((item) => {
    if (!item || typeof item.id !== 'string' || item.id.trim().length === 0) {
      return;
    }
    const gridCols = isFiniteNumber(item.gridCols) ? clampInt(item.gridCols, 1) : defaultCols;
    const rowHeightPx = isFiniteNumber(item.rowHeightPx) ? clampInt(item.rowHeightPx, 1) : defaultRowHeight;
    const widthUnits = clampInt(item.w, 1, gridCols);
    const heightUnits = clampInt(item.h, 1);
    const maxX = Math.max(0, gridCols - widthUnits);
    const x = clampInt(item.x, 0, maxX);
    const y = clampInt(item.y, 0);
    tiles[item.id] = {
      layout: {
        x,
        y,
        w: widthUnits,
        h: heightUnits,
      },
      gridCols,
      rowHeightPx,
      layoutVersion: isFiniteNumber(item.layoutVersion)
        ? clampInt(item.layoutVersion, 0)
        : options?.layoutVersion ?? GRID_LAYOUT_VERSION,
      widthPx: optionalNumber(item.widthPx),
      heightPx: optionalNumber(item.heightPx),
      zoom: optionalNumber(item.zoom),
      locked: optionalBoolean(item.locked),
      toolbarPinned: optionalBoolean(item.toolbarPinned),
      manualLayout: true,
      measurementVersion: undefined,
      measurementSource: null,
      hostCols: undefined,
      hostRows: undefined,
    };
  });
  return {
    tiles,
    gridCols: defaultCols,
    rowHeightPx: defaultRowHeight,
    layoutVersion: options?.layoutVersion ?? GRID_LAYOUT_VERSION,
  };
}

export function reactGridToGridSnapshot(
  layouts: ReactGridLayoutItem[],
  options: {
    cols: number;
    rowHeightPx?: number;
    layoutVersion?: number;
    previous?: GridLayoutSnapshot | null | undefined;
  },
): GridLayoutSnapshot {
  const tiles: GridLayoutSnapshot['tiles'] = {};
  const previous = options.previous ?? null;
  const cols = Math.max(1, Math.floor(options.cols));
  layouts.forEach((item) => {
    const id = typeof item?.i === 'string' ? item.i : '';
    if (!id) return;
    const prev = previous?.tiles[id];
    const widthUnits = clampInt(item.w, 1, cols);
    const maxX = Math.max(0, cols - widthUnits);
    tiles[id] = {
      layout: {
        x: clampInt(item.x, 0, maxX),
        y: clampInt(item.y, 0),
        w: widthUnits,
        h: clampInt(item.h, 1),
      },
      gridCols: cols,
      rowHeightPx: options.rowHeightPx ?? prev?.rowHeightPx ?? DEFAULT_ROW_HEIGHT_PX,
      layoutVersion: options.layoutVersion ?? prev?.layoutVersion ?? GRID_LAYOUT_VERSION,
      widthPx: prev?.widthPx,
      heightPx: prev?.heightPx,
      zoom: prev?.zoom,
      locked: prev?.locked,
      toolbarPinned: prev?.toolbarPinned,
      manualLayout: true,
      hostCols: prev?.hostCols,
      hostRows: prev?.hostRows,
      measurementVersion: prev?.measurementVersion,
      measurementSource: prev?.measurementSource ?? null,
    };
  });
  return {
    tiles,
    gridCols: cols,
    rowHeightPx: options.rowHeightPx ?? previous?.rowHeightPx ?? DEFAULT_ROW_HEIGHT_PX,
    layoutVersion: options.layoutVersion ?? previous?.layoutVersion ?? GRID_LAYOUT_VERSION,
  };
}

export function gridSnapshotToBeachItems(
  layout: SharedCanvasLayout,
  options?: { fallbackCols?: number; fallbackRowHeightPx?: number },
): BeachLayoutItem[] {
  const snapshot = extractGridLayoutSnapshot(layout);
  const fallbackCols = options?.fallbackCols ?? snapshot.gridCols ?? DEFAULT_GRID_COLS;
  const fallbackRowHeight = options?.fallbackRowHeightPx ?? snapshot.rowHeightPx ?? DEFAULT_ROW_HEIGHT_PX;
  const items: BeachLayoutItem[] = [];
  for (const [tileId, tile] of Object.entries(layout.tiles)) {
    const metadata = snapshot.tiles[tileId] ?? extractGridDashboardMetadata(tile);
    const gridCols = metadata.gridCols ?? fallbackCols;
    const rowHeightPx = metadata.rowHeightPx ?? fallbackRowHeight;
    items.push({
      id: tileId,
      x: metadata.layout.x,
      y: metadata.layout.y,
      w: metadata.layout.w,
      h: metadata.layout.h,
      widthPx: metadata.widthPx ?? Math.round(tile.size?.width ?? 0),
      heightPx: metadata.heightPx ?? Math.round(tile.size?.height ?? 0),
      zoom: metadata.zoom,
      locked: metadata.locked,
      toolbarPinned: metadata.toolbarPinned,
      gridCols,
      rowHeightPx,
      layoutVersion: metadata.layoutVersion ?? snapshot.layoutVersion ?? GRID_LAYOUT_VERSION,
    });
  }
  return items;
}

export function withLayoutDashboardMetadata(
  layout: SharedCanvasLayout,
  snapshot: GridLayoutSnapshot,
): SharedCanvasLayout {
  const previous = isRecord((layout.metadata as any)?.dashboard)
    ? ((layout.metadata as any).dashboard as Record<string, unknown>)
    : {};
  const gridCols =
    snapshot.gridCols ?? (isFiniteNumber(previous.gridCols) ? (previous.gridCols as number) : DEFAULT_GRID_COLS);
  const rowHeightPx =
    snapshot.rowHeightPx ??
    (isFiniteNumber(previous.rowHeightPx) ? (previous.rowHeightPx as number) : DEFAULT_ROW_HEIGHT_PX);
  const layoutVersion =
    snapshot.layoutVersion ??
    (isFiniteNumber(previous.layoutVersion) ? (previous.layoutVersion as number) : GRID_LAYOUT_VERSION);
  if (
    previous.gridCols === gridCols &&
    previous.rowHeightPx === rowHeightPx &&
    previous.layoutVersion === layoutVersion
  ) {
    return layout;
  }
  return {
    ...layout,
    metadata: {
      ...layout.metadata,
      dashboard: {
        ...previous,
        gridCols,
        rowHeightPx,
        layoutVersion,
      },
    },
  };
}

export function gridSnapshotToReactGrid(
  layout: SharedCanvasLayout,
  options?: { fallbackCols?: number; minW?: number; minH?: number },
): ReactGridLayoutItem[] {
  const snapshot = extractGridLayoutSnapshot(layout);
  const minW = options?.minW ?? 1;
  const minH = options?.minH ?? 1;
  const fallbackCols = options?.fallbackCols ?? snapshot.gridCols ?? DEFAULT_GRID_COLS;
  return Object.entries(layout.tiles).map(([tileId, tile]) => {
    const metadata = snapshot.tiles[tileId] ?? extractGridDashboardMetadata(tile);
    const clampedWidth = Math.min(metadata.layout.w, fallbackCols);
    const maxX = Math.max(0, fallbackCols - clampedWidth);
    const x = Math.min(metadata.layout.x, maxX);
    const normalized: ReactGridLayoutItem = {
      i: tileId,
      x,
      y: metadata.layout.y,
      w: clampedWidth,
      h: metadata.layout.h,
      minW,
      minH,
      static: Boolean(metadata.locked),
    };
    return normalized;
  });
}
