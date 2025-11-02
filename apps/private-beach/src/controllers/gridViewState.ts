'use client';

export type TileMeasurements = {
  width: number;
  height: number;
};

export type PreviewMetrics = {
  scale: number;
  targetWidth: number;
  targetHeight: number;
  rawWidth: number;
  rawHeight: number;
  hostRows: number | null;
  hostCols: number | null;
  measurementVersion: number;
  hostRowSource?: 'unknown' | 'pty' | 'fallback';
  hostColSource?: 'unknown' | 'pty' | 'fallback';
};

export type TileViewState = {
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

export function defaultTileViewState(): TileViewState {
  return {
    zoom: 1,
    locked: false,
    toolbarPinned: false,
    measurements: null,
    hostCols: null,
    hostRows: null,
    hasHostDimensions: false,
    viewportCols: null,
    viewportRows: null,
    lastLayout: null,
    layoutInitialized: false,
    manualLayout: false,
    layoutHostCols: null,
    layoutHostRows: null,
    previewStatus: 'connecting',
    preview: null,
  };
}

export function mergeTileViewState(base: TileViewState, updates: Partial<TileViewState>): TileViewState {
  if (!updates || Object.keys(updates).length === 0) {
    return base;
  }
  const has = (key: keyof TileViewState) => Object.prototype.hasOwnProperty.call(updates, key);
  const next: TileViewState = { ...base };

  if (has('zoom') && typeof updates.zoom === 'number') {
    next.zoom = updates.zoom;
  }
  if (has('locked') && typeof updates.locked === 'boolean') {
    next.locked = updates.locked;
  }
  if (has('toolbarPinned') && typeof updates.toolbarPinned === 'boolean') {
    next.toolbarPinned = updates.toolbarPinned;
  }
  if (has('measurements')) {
    next.measurements = updates.measurements ?? null;
  }
  if (has('hostCols')) {
    next.hostCols = updates.hostCols ?? null;
  }
  if (has('hostRows')) {
    next.hostRows = updates.hostRows ?? null;
  }
  if (has('hasHostDimensions') && typeof updates.hasHostDimensions === 'boolean') {
    next.hasHostDimensions = updates.hasHostDimensions;
  }
  if (has('viewportCols')) {
    next.viewportCols = updates.viewportCols ?? null;
  }
  if (has('viewportRows')) {
    next.viewportRows = updates.viewportRows ?? null;
  }
  if (has('lastLayout')) {
    next.lastLayout = updates.lastLayout ?? null;
  }
  if (has('layoutInitialized') && typeof updates.layoutInitialized === 'boolean') {
    next.layoutInitialized = updates.layoutInitialized;
  }
  if (has('manualLayout') && typeof updates.manualLayout === 'boolean') {
    next.manualLayout = updates.manualLayout;
  }
  if (has('layoutHostCols')) {
    next.layoutHostCols = updates.layoutHostCols ?? null;
  }
  if (has('layoutHostRows')) {
    next.layoutHostRows = updates.layoutHostRows ?? null;
  }
  if (has('previewStatus') && typeof updates.previewStatus === 'string') {
    next.previewStatus = updates.previewStatus as TileViewState['previewStatus'];
  }
  if (has('preview')) {
    next.preview = updates.preview ?? null;
  }

  return next;
}
