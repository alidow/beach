'use client';

import { useSyncExternalStore } from 'react';
import { fetchSessionStateSnapshot, type SessionSummary, type BeachLayoutItem } from '../lib/api';
import type { CanvasLayout as ApiCanvasLayout } from '../lib/api';
import type { CanvasLayout as SharedCanvasLayout, CanvasTileNode } from '../canvas';
import type { SessionCredentialOverride, TerminalViewerState } from '../hooks/terminalViewerTypes';
import type { TerminalStateDiff } from '../lib/terminalHydrator';
import { emitTelemetry } from '../lib/telemetry';
import { viewerConnectionService } from './viewerConnectionService';
import {
  applyGridMetadataToLayout,
  beachItemsToGridSnapshot,
  extractGridDashboardMetadata,
  extractGridLayoutSnapshot,
  gridSnapshotToBeachItems,
  DEFAULT_GRID_W_UNITS,
  DEFAULT_GRID_H_UNITS,
  withLayoutDashboardMetadata,
  withTileGridMetadata,
  type GridDashboardMetadata,
  type GridLayoutSnapshot,
  type TileGridMetadataUpdate,
} from './gridLayout';
import type { GridCommandResult } from './gridLayoutCommands';
import { defaultTileViewState, type TileViewState } from './gridViewState';

const DEFAULT_TILE_WIDTH = 448;
const DEFAULT_TILE_HEIGHT = 448;
const PERSIST_DELAY_MS = 200;
const MEASUREMENT_DEBOUNCE_MS = 32;

export type TileMeasurementPayload = {
  scale: number;
  targetWidth: number;
  targetHeight: number;
  rawWidth: number;
  rawHeight: number;
  hostRows: number | null;
  hostCols: number | null;
  measurementVersion: number;
};

type MeasurementSource = 'dom' | 'host';

type MeasurementCommand = {
  tileId: string;
  payload: TileMeasurementPayload;
  source: MeasurementSource;
  signature: string;
  timestamp: number;
};

type TileViewStateUpdate =
  | Partial<TileViewState>
  | ((current: TileViewState) => Partial<TileViewState> | null | undefined);

type TileSnapshot = {
  tileId: string;
  layout: CanvasTileNode | null;
  session: SessionSummary | null;
  viewer: TerminalViewerState;
  cachedDiff: TerminalStateDiff | null;
  measurementVersion: number;
  grid: GridDashboardMetadata;
};

type ControllerSnapshot = {
  layout: SharedCanvasLayout;
  version: number;
  updatedAt: number;
};

type HydrateInput = {
  layout: ApiCanvasLayout | SharedCanvasLayout | null;
  gridLayoutItems?: BeachLayoutItem[] | null | undefined;
  gridLayoutSnapshot?: GridLayoutSnapshot | null | undefined;
  sessions: SessionSummary[];
  agents: SessionSummary[];
  privateBeachId: string | null;
  managerUrl: string;
  managerToken: string | null;
  viewerToken?: string | null;
  viewerOverrides?: Record<string, SessionCredentialOverride | null | undefined>;
  viewerStateOverrides?: Record<string, TerminalViewerState | null | undefined>;
  cachedDiffs?: Record<string, TerminalStateDiff | null | undefined>;
  onPersistLayout?: (layout: ApiCanvasLayout) => void;
  onLayoutChange?: (layout: ApiCanvasLayout) => void;
};

const IDLE_VIEWER_STATE: TerminalViewerState & { transportVersion?: number } = {
  store: null,
  transport: null,
  connecting: false,
  error: null,
  status: 'idle',
  secureSummary: null,
  latencyMs: null,
  transportVersion: 0,
};

function cloneViewerState(state: TerminalViewerState): TerminalViewerState {
  return {
    store: state.store,
    transport: state.transport,
    connecting: state.connecting,
    error: state.error,
    status: state.status,
    secureSummary: state.secureSummary,
    latencyMs: state.latencyMs,
    transportVersion: (state as any).transportVersion ?? 0,
  };
}

function ensureLayout(input: ApiCanvasLayout | SharedCanvasLayout | null): SharedCanvasLayout {
  const now = Date.now();
  if (!input) {
    return {
      version: 3,
      viewport: { zoom: 1, pan: { x: 0, y: 0 } },
      tiles: {},
      groups: {},
      agents: {},
      controlAssignments: {},
      metadata: { createdAt: now, updatedAt: now },
    };
  }
  const rawTiles = input.tiles ?? {};
  const tiles: SharedCanvasLayout['tiles'] = {};
  for (const [tileId, tile] of Object.entries(rawTiles)) {
    tiles[tileId] = {
      ...tile,
      id: tile.id ?? tileId,
      kind: tile.kind ?? 'application',
      position: tile.position ?? { x: 0, y: 0 },
      size: tile.size ?? { width: DEFAULT_TILE_WIDTH, height: DEFAULT_TILE_HEIGHT },
      zIndex: tile.zIndex ?? 1,
      metadata: tile.metadata ?? {},
    };
  }
  return {
    version: 3,
    viewport: input.viewport ?? { zoom: 1, pan: { x: 0, y: 0 } },
    tiles,
    groups: input.groups ?? {},
    agents: input.agents ?? {},
    controlAssignments: input.controlAssignments ?? {},
    metadata: {
      createdAt: input.metadata?.createdAt ?? now,
      updatedAt: input.metadata?.updatedAt ?? now,
      migratedFrom: input.metadata?.migratedFrom,
    },
  };
}

function withUpdatedTimestamp(layout: SharedCanvasLayout, timestamp = Date.now()): SharedCanvasLayout {
  return {
    ...layout,
    metadata: {
      ...layout.metadata,
      updatedAt: timestamp,
    },
  };
}

function buildMeasurementSignature(payload: TileMeasurementPayload, source: MeasurementSource): string {
  return JSON.stringify([
    Math.round(payload.rawWidth * 100) / 100,
    Math.round(payload.rawHeight * 100) / 100,
    Math.round(payload.scale * 1000) / 1000,
    payload.hostRows ?? null,
    payload.hostCols ?? null,
    payload.measurementVersion,
    source,
  ]);
}

class SessionTileController {
  private layout: SharedCanvasLayout = ensureLayout(null);
  private snapshot: ControllerSnapshot = {
    layout: this.layout,
    version: 0,
    updatedAt: Date.now(),
  };
  private controllerVersion = 0;
  private subscribers = new Set<() => void>();
  private tileSubscribers = new Map<string, Set<() => void>>();
  private tileSnapshots = new Map<string, TileSnapshot>();
  private sessions = new Map<string, SessionSummary>();
  private agents = new Map<string, SessionSummary>();
  private viewerOverrides = new Map<string, SessionCredentialOverride | null>();
  private viewerStateOverrides = new Map<string, TerminalViewerState | null>();
  private cachedDiffs = new Map<string, TerminalStateDiff | null>();
  private snapshotFetches = new Map<string, Promise<void>>();
  private measurementQueue = new Map<string, MeasurementCommand>();
  private measurementTimer: ReturnType<typeof setTimeout> | null = null;
  private measurementSignatures = new Map<string, string>();
  private persistTimer: ReturnType<typeof setTimeout> | null = null;
  private persistCallback: ((layout: ApiCanvasLayout) => void) | null = null;
  private pendingGridLayoutOverride = new Map<string, GridLayoutUnits>();
  private privateBeachId: string | null = null;
  private managerUrl = '';
  private managerToken: string | null = null;
  private viewerToken: string | null = null;
  private connectionHandles = new Map<string, () => void>();
  private layoutChangeCallback: ((layout: ApiCanvasLayout) => void) | null = null;

  hydrate(input: HydrateInput) {
    this.privateBeachId = input.privateBeachId ?? null;
    this.managerUrl = input.managerUrl?.trim() ?? '';
    this.managerToken = input.managerToken?.trim() ?? null;
    this.viewerToken = input.viewerToken?.trim() ?? null;

    if (input.onPersistLayout) {
      this.persistCallback = input.onPersistLayout;
    } else {
      this.persistCallback = null;
    }
    this.layoutChangeCallback = input.onLayoutChange ?? null;

    this.sessions = new Map(input.sessions.map((session) => [session.session_id, session]));
    this.agents = new Map(input.agents.map((session) => [session.session_id, session]));

    this.viewerOverrides.clear();
    if (input.viewerOverrides) {
      for (const [key, value] of Object.entries(input.viewerOverrides)) {
        this.viewerOverrides.set(key, value ?? null);
      }
    }

    this.viewerStateOverrides.clear();
    if (input.viewerStateOverrides) {
      for (const [key, value] of Object.entries(input.viewerStateOverrides)) {
        this.viewerStateOverrides.set(key, value ?? null);
      }
    }

    this.cachedDiffs.clear();
    if (input.cachedDiffs) {
      for (const [key, value] of Object.entries(input.cachedDiffs)) {
        this.cachedDiffs.set(key, value ?? null);
      }
    }
    this.snapshotFetches.clear();

    const ensuredLayout = ensureLayout(input.layout);
    let resolvedLayout = ensuredLayout;
    if (input.gridLayoutSnapshot) {
      resolvedLayout = this.applyGridSnapshotToLayout(ensuredLayout, input.gridLayoutSnapshot);
    } else if (input.gridLayoutItems && input.gridLayoutItems.length > 0) {
      const snapshot = beachItemsToGridSnapshot(input.gridLayoutItems);
      resolvedLayout = this.applyGridSnapshotToLayout(ensuredLayout, snapshot);
    }
    this.replaceLayout(resolvedLayout, { reason: 'hydrate', suppressPersist: true });
    this.syncTileConnections();
  }

  subscribe(listener: () => void): () => void {
    this.subscribers.add(listener);
    return () => {
      this.subscribers.delete(listener);
    };
  }

  subscribeTile(tileId: string, listener: () => void): () => void {
    const existing = this.tileSubscribers.get(tileId);
    if (existing) {
      existing.add(listener);
    } else {
      this.tileSubscribers.set(tileId, new Set([listener]));
    }
    return () => {
      const set = this.tileSubscribers.get(tileId);
      if (!set) return;
      set.delete(listener);
      if (set.size === 0) {
        this.tileSubscribers.delete(tileId);
      }
    };
  }

  getSnapshot(): ControllerSnapshot {
    return this.snapshot;
  }

  getTileSnapshot(tileId: string): TileSnapshot {
    let snapshot = this.tileSnapshots.get(tileId);
    if (snapshot) {
      return snapshot;
    }
    const tileNode = this.layout.tiles[tileId] ?? null;
    const measurementVersion =
      typeof tileNode?.metadata?.measurementVersion === 'number'
        ? (tileNode?.metadata?.measurementVersion as number)
        : 0;
    snapshot = {
      tileId,
      layout: tileNode,
      session: this.sessions.get(tileId) ?? null,
      viewer: cloneViewerState(IDLE_VIEWER_STATE),
      cachedDiff: this.cachedDiffs.get(tileId) ?? null,
      measurementVersion,
      grid: extractGridDashboardMetadata(tileNode),
    };
    this.tileSnapshots.set(tileId, snapshot);
    return snapshot;
  }

  updateLayout(
    reason: string,
    mutate: (layout: SharedCanvasLayout) => SharedCanvasLayout | null | undefined,
    options?: { suppressPersist?: boolean; skipLayoutChange?: boolean; preserveUpdatedAt?: boolean },
  ) {
    const base = this.layout;
    const produced = mutate(base);
    if (!produced) {
      return;
    }
    const ensured = ensureLayout(produced);
    if (Object.is(ensured, base)) {
      return;
    }
    this.replaceLayout(withUpdatedTimestamp(ensured), { reason, ...options });
  }

  applyGridSnapshot(reason: string, snapshot: GridLayoutSnapshot | null | undefined, options?: { suppressPersist?: boolean }) {
    if (!snapshot) {
      return;
    }
    const produced = this.applyGridSnapshotToLayout(this.layout, snapshot);
    if (!options?.suppressPersist) {
      for (const [tileId, metadata] of Object.entries(snapshot.tiles)) {
        const layout = metadata.layout;
        if (!layout) {
          continue;
        }
        const override: GridLayoutUnits = {
          x: typeof layout.x === 'number' ? layout.x : 0,
          y: typeof layout.y === 'number' ? layout.y : 0,
          w: typeof layout.w === 'number' ? layout.w : DEFAULT_GRID_W_UNITS,
          h: typeof layout.h === 'number' ? layout.h : DEFAULT_GRID_H_UNITS,
        };
        this.pendingGridLayoutOverride.set(tileId, override);
      }
    }
    if (options?.suppressPersist) {
      if (Object.is(produced, this.layout)) {
        return;
      }
      this.replaceLayout(withUpdatedTimestamp(produced), { reason, suppressPersist: true });
      return;
    }
    if (Object.is(produced, this.layout)) {
      this.schedulePersist();
      return;
    }
    this.updateLayout(reason, () => produced);
  }

  applyGridCommand(
    reason: string,
    executor: (layout: SharedCanvasLayout) => GridCommandResult | null | undefined,
    options?: { suppressPersist?: boolean },
  ) {
    if (options?.suppressPersist) {
      const result = executor(this.layout);
      if (!result || !result.mutated) {
        return;
      }
      const produced = this.applyGridSnapshotToLayout(this.layout, result.snapshot);
      if (Object.is(produced, this.layout)) {
        return;
      }
      this.replaceLayout(withUpdatedTimestamp(produced), { reason, suppressPersist: true });
      return;
    }
    this.updateLayout(reason, (layout) => {
      const result = executor(layout);
      if (!result || !result.mutated) {
        return layout;
      }
      return this.applyGridSnapshotToLayout(layout, result.snapshot);
    });
  }

  requestPersist() {
    this.schedulePersist();
  }

  updateTileGridMetadata(
    tileId: string,
    reason: string,
    updater: TileGridMetadataUpdate | ((current: GridDashboardMetadata) => TileGridMetadataUpdate | null | undefined),
    options?: { suppressPersist?: boolean },
  ) {
    this.updateLayout(reason, (layout) => {
      const existingTile = layout.tiles[tileId];
      const baseTile =
        existingTile ??
        ({
          id: tileId,
          kind: 'application',
          position: { x: 0, y: 0 },
          size: { width: DEFAULT_TILE_WIDTH, height: DEFAULT_TILE_HEIGHT },
          zIndex: 1,
          metadata: {},
        } as CanvasTileNode);
      const currentMeta = extractGridDashboardMetadata(baseTile);
      const update = typeof updater === 'function' ? updater(currentMeta) : updater;
      if (!update) {
        return layout;
      }
      const nextTile = withTileGridMetadata(baseTile, update);
      if (nextTile === existingTile) {
        return layout;
      }
      return {
        ...layout,
        tiles: {
          ...layout.tiles,
          [tileId]: nextTile,
        },
      };
    }, options);
  }

  updateTileViewState(tileId: string, reason: string, updater: TileViewStateUpdate, options?: { persist?: boolean }) {
    const persist = options?.persist ?? true;
    this.updateTileGridMetadata(tileId, reason, (current) => {
      const base = current.viewState ?? defaultTileViewState();
      const patch = typeof updater === 'function' ? updater(base) : updater;
      if (!patch || Object.keys(patch).length === 0) {
        return null;
      }
      return {
        viewState: patch,
      };
    }, { suppressPersist: !persist });
  }

  setTileLocked(tileId: string, locked: boolean) {
    this.updateTileViewState(tileId, locked ? 'view-state.lock.enable' : 'view-state.lock.disable', { locked });
  }

  setTileToolbarPinned(tileId: string, pinned: boolean) {
    this.updateTileViewState(tileId, pinned ? 'view-state.toolbar.pin' : 'view-state.toolbar.unpin', {
      toolbarPinned: pinned,
    });
  }

  setTilePreviewStatus(tileId: string, status: TileViewState['previewStatus']) {
    this.updateTileViewState(tileId, 'view-state.preview-status', { previewStatus: status }, { persist: false });
  }

  setTilePreview(tileId: string, preview: TileViewState['preview']) {
    this.updateTileViewState(tileId, 'view-state.preview', { preview }, { persist: false });
  }

  hydrateGridLayoutFromBeachItems(items: BeachLayoutItem[] | null | undefined) {
    if (!items || items.length === 0) {
      return;
    }
    const snapshot = beachItemsToGridSnapshot(items);
    this.applyGridSnapshot('hydrate-grid', snapshot, { suppressPersist: true });
  }

  getGridLayoutSnapshot(): GridLayoutSnapshot {
    return extractGridLayoutSnapshot(this.layout);
  }

  exportGridLayoutAsBeachItems(): BeachLayoutItem[] {
    const items = gridSnapshotToBeachItems(this.layout);
    if (this.pendingGridLayoutOverride.size === 0) {
      return items;
    }
    const overridden = items.map((item) => {
      const override = this.pendingGridLayoutOverride.get(item.id);
      if (!override) {
        return item;
      }
      return {
        ...item,
        x: override.x,
        y: override.y,
      };
    });
    this.pendingGridLayoutOverride.clear();
    return overridden;
  }

  enqueueMeasurement(tileId: string, payload: TileMeasurementPayload | null | undefined, source: MeasurementSource = 'dom') {
    if (!payload) {
      return;
    }
    const tile = this.layout.tiles[tileId];
    if (source === 'dom' && tile) {
      const tileVersion =
        typeof tile.metadata?.measurementVersion === 'number' ? (tile.metadata?.measurementVersion as number) : 0;
      const tileSource = (tile.metadata?.measurementSource as MeasurementSource | undefined) ?? 'dom';
      if (tileSource === 'host' && tileVersion >= payload.measurementVersion) {
        emitTelemetry('canvas.measurement.dom-skipped-after-host', {
          beachId: this.privateBeachId ?? undefined,
          sessionId: tileId,
          domVersion: payload.measurementVersion,
          appliedVersion: tileVersion,
          stage: 'enqueue',
        });
        return;
      }
    }
    const signature = buildMeasurementSignature(payload, source);
    const appliedSignature = this.measurementSignatures.get(tileId);
    if (appliedSignature === signature) {
      return;
    }
    const existing = this.measurementQueue.get(tileId);
    if (existing) {
      if (existing.signature === signature) {
        return;
      }
      if (existing.source === 'host' && source === 'dom') {
        emitTelemetry('canvas.measurement.dom-skipped-after-host', {
          beachId: this.privateBeachId ?? undefined,
          sessionId: tileId,
          domVersion: payload.measurementVersion,
          appliedVersion: existing.payload.measurementVersion,
          stage: 'queue',
        });
        return;
      }
      if (existing.source === 'dom' && source === 'host') {
        emitTelemetry('canvas.measurement.dom-skipped-after-host', {
          beachId: this.privateBeachId ?? undefined,
          sessionId: tileId,
          domVersion: existing.payload.measurementVersion,
          appliedVersion: payload.measurementVersion,
          stage: 'queue-preempted',
        });
      }
    }
    this.measurementQueue.set(tileId, {
      tileId,
      payload,
      source,
      signature,
      timestamp: Date.now(),
    });
    this.scheduleMeasurementFlush();
  }

  applyHostDimensions(tileId: string, payload: TileMeasurementPayload) {
    this.enqueueMeasurement(tileId, payload, 'host');
  }

  setCachedDiff(tileId: string, diff: TerminalStateDiff | null) {
    const prev = this.cachedDiffs.get(tileId) ?? null;
    const normalized = diff ?? null;
    if (prev === normalized) {
      return;
    }
    this.cachedDiffs.set(tileId, normalized);
    this.updateTileSnapshot(tileId, (current) => ({
      ...current,
      cachedDiff: normalized,
    }));
  }

  setViewerOverride(tileId: string, state: TerminalViewerState | null) {
    const existing = this.viewerStateOverrides.get(tileId) ?? null;
    if (existing === state) {
      return;
    }
    if (state) {
      this.viewerStateOverrides.set(tileId, state);
    } else {
      this.viewerStateOverrides.delete(tileId);
    }
    this.updateTileSnapshot(tileId, (current) => ({
      ...current,
      viewer: state ? cloneViewerState(state) : current.viewer,
    }));
    this.syncTileConnections();
  }

  handleViewerSnapshot(tileId: string, snapshot: TerminalViewerState) {
    const override = this.viewerStateOverrides.get(tileId);
    if (override) {
      return;
    }
    const existing = this.tileSnapshots.get(tileId);
    if (existing && shallowViewerEqual(existing.viewer, snapshot)) {
      return;
    }
    this.updateTileSnapshot(tileId, (current) => ({
      ...current,
      viewer: cloneViewerState(snapshot),
    }));
  }

  getTileMetrics(tileId: string) {
    return viewerConnectionService.getTileMetrics(tileId);
  }

  resetViewerMetrics() {
    viewerConnectionService.resetMetrics();
  }

  private replaceLayout(
    nextLayout: SharedCanvasLayout,
    options?: { reason?: string; suppressPersist?: boolean; skipLayoutChange?: boolean; preserveUpdatedAt?: boolean },
  ) {
    const prevTiles = new Set(Object.keys(this.layout.tiles));
    const nextTiles = new Set(Object.keys(nextLayout.tiles));
    const removed = [...prevTiles].filter((id) => !nextTiles.has(id));

    for (const removedId of removed) {
      const handle = this.connectionHandles.get(removedId);
      if (handle) {
        handle();
        this.connectionHandles.delete(removedId);
      }
      this.tileSnapshots.delete(removedId);
      this.measurementSignatures.delete(removedId);
      viewerConnectionService.disconnectTile(removedId);
      this.snapshotFetches.delete(removedId);
      this.emitTile(removedId);
    }

    this.layout = nextLayout;
    this.controllerVersion += 1;
    const snapshotUpdatedAt = options?.preserveUpdatedAt ? this.snapshot.updatedAt : Date.now();
    this.snapshot = {
      layout: this.layout,
      version: this.controllerVersion,
      updatedAt: snapshotUpdatedAt,
    };
    if (!options?.skipLayoutChange && options?.reason !== 'hydrate') {
      this.layoutChangeCallback?.(nextLayout as ApiCanvasLayout);
    }

    for (const tileId of nextTiles) {
      const tile = this.layout.tiles[tileId];
      const session = this.sessions.get(tileId) ?? null;
      const overrideViewer = this.viewerStateOverrides.get(tileId) ?? null;
      const cachedDiff = this.cachedDiffs.get(tileId) ?? null;
      const measurementVersion =
        typeof tile.metadata?.measurementVersion === 'number' ? tile.metadata?.measurementVersion : 0;
      const viewer = overrideViewer ? cloneViewerState(overrideViewer) : this.tileSnapshots.get(tileId)?.viewer ?? cloneViewerState(IDLE_VIEWER_STATE);
      this.tileSnapshots.set(tileId, {
        tileId,
        layout: tile,
        session,
        viewer,
        cachedDiff,
        measurementVersion,
        grid: extractGridDashboardMetadata(tile),
      });
      this.emitTile(tileId);
    }

    this.emit();

    if (!options?.suppressPersist) {
      this.schedulePersist();
    }

    this.syncTileConnections();
  }

  private scheduleMeasurementFlush() {
    if (this.measurementTimer) {
      return;
    }
    this.measurementTimer = setTimeout(() => {
      this.measurementTimer = null;
      this.flushMeasurements();
    }, MEASUREMENT_DEBOUNCE_MS);
  }

  private flushMeasurements() {
    const commands = Array.from(this.measurementQueue.values());
    if (commands.length === 0) {
      return;
    }
    this.measurementQueue.clear();
    let didMutate = false;
    let layout = this.layout;

    for (const command of commands) {
      const { tileId, payload, source, signature } = command;
      const tile = layout.tiles[tileId];
      if (!tile) {
        continue;
      }
      const currentVersion =
        typeof tile.metadata?.measurementVersion === 'number' ? (tile.metadata?.measurementVersion as number) : 0;
      const existingSource = (tile.metadata?.measurementSource as MeasurementSource | undefined) ?? 'dom';
      if (payload.measurementVersion < currentVersion) {
        if (source === 'dom' && existingSource === 'host') {
          emitTelemetry('canvas.measurement.dom-skipped-after-host', {
            beachId: this.privateBeachId ?? undefined,
            sessionId: tileId,
            domVersion: payload.measurementVersion,
            appliedVersion: currentVersion,
            stage: 'flush',
          });
        }
        continue;
      }
      if (payload.measurementVersion === currentVersion && source !== 'host') {
        if (source === 'dom' && existingSource === 'host') {
          emitTelemetry('canvas.measurement.dom-skipped-after-host', {
            beachId: this.privateBeachId ?? undefined,
            sessionId: tileId,
            domVersion: payload.measurementVersion,
            appliedVersion: currentVersion,
            stage: 'flush',
          });
        }
        continue;
      }
      if (source === 'dom' && existingSource === 'host' && payload.measurementVersion > currentVersion) {
        emitTelemetry('canvas.measurement.dom-advanced-after-host', {
          beachId: this.privateBeachId ?? undefined,
          sessionId: tileId,
          domVersion: payload.measurementVersion,
          appliedVersion: currentVersion,
          stage: 'flush',
        });
      }
      const width = Math.max(1, Math.round(payload.rawWidth));
      const height = Math.max(1, Math.round(payload.rawHeight));
      const existingWidth = Math.round(tile.size?.width ?? 0);
      const existingHeight = Math.round(tile.size?.height ?? 0);
      const existingRawWidth = tile.metadata?.rawWidth ?? null;
      const existingRawHeight = tile.metadata?.rawHeight ?? null;
      const existingScale = tile.metadata?.scale ?? null;
      const existingHostRows = tile.metadata?.hostRows ?? null;
      const existingHostCols = tile.metadata?.hostCols ?? null;

      const hasSameMetrics =
        existingWidth === width &&
        existingHeight === height &&
        existingRawWidth === payload.rawWidth &&
        existingRawHeight === payload.rawHeight &&
        existingScale === payload.scale &&
        existingHostRows === payload.hostRows &&
        existingHostCols === payload.hostCols;

      if (hasSameMetrics && existingSource === source) {
        this.measurementSignatures.set(tileId, signature);
        continue;
      }

      const metadata = {
        ...(tile.metadata ?? {}),
        measurementVersion: payload.measurementVersion,
        rawWidth: payload.rawWidth,
        rawHeight: payload.rawHeight,
        scale: payload.scale,
        hostRows: payload.hostRows,
        hostCols: payload.hostCols,
        measurementSource: source,
      };

      let nextTile: CanvasTileNode = {
        ...tile,
        size: { width, height },
        metadata,
      };
      nextTile = withTileGridMetadata(nextTile, {
        widthPx: width,
        heightPx: height,
        hostCols: payload.hostCols ?? null,
        hostRows: payload.hostRows ?? null,
        measurementVersion: payload.measurementVersion,
        measurementSource: source,
      });

      layout = {
        ...layout,
        tiles: {
          ...layout.tiles,
          [tileId]: nextTile,
        },
      };

      this.measurementSignatures.set(tileId, signature);
      didMutate = true;

      const shouldEmitMeasurementTelemetry =
        !hasSameMetrics || payload.measurementVersion > currentVersion;
      const shouldEmitResizeTelemetry = existingWidth !== width || existingHeight !== height;

      if (shouldEmitMeasurementTelemetry) {
        emitTelemetry('canvas.measurement', {
          beachId: this.privateBeachId ?? undefined,
          sessionId: tileId,
          targetWidth: payload.targetWidth,
          targetHeight: payload.targetHeight,
          rawWidth: payload.rawWidth,
          rawHeight: payload.rawHeight,
          scale: payload.scale,
          measurementVersion: payload.measurementVersion,
        });
      }

      if (shouldEmitResizeTelemetry) {
        emitTelemetry('canvas.resize.stop', {
          beachId: this.privateBeachId ?? undefined,
          sessionId: tileId,
          width,
          height,
        });
      }
    }

    if (didMutate) {
      this.replaceLayout(layout, {
        reason: 'measurement',
        suppressPersist: true,
        skipLayoutChange: true,
        preserveUpdatedAt: true,
      });
    }
  }

  private syncTileConnections() {
    for (const [tileId, snapshot] of this.tileSnapshots) {
      const override = this.viewerStateOverrides.get(tileId) ?? null;
      if (override) {
        const handle = this.connectionHandles.get(tileId);
        if (handle) {
          handle();
          this.connectionHandles.delete(tileId);
        }
        viewerConnectionService.disconnectTile(tileId);
        continue;
      }
      const session = snapshot.session ?? this.sessions.get(tileId) ?? null;
      if (!session) {
        viewerConnectionService.disconnectTile(tileId);
        continue;
      }
      if (!this.managerUrl || this.managerUrl.length === 0) {
        viewerConnectionService.disconnectTile(tileId);
        continue;
      }
      const authToken = this.viewerToken ?? this.managerToken;
      this.scheduleSnapshotFetch(tileId, session.session_id);
      const handle = viewerConnectionService.connectTile(
        tileId,
        {
          sessionId: session.session_id,
          privateBeachId: session.private_beach_id ?? this.privateBeachId,
          managerUrl: this.managerUrl,
          authToken,
          override: this.viewerOverrides.get(tileId) ?? undefined,
        },
        (state) => this.handleViewerSnapshot(tileId, state),
      );
      this.connectionHandles.set(tileId, handle);
    }
  }

  private scheduleSnapshotFetch(tileId: string, sessionId: string) {
    const existingDiff = this.cachedDiffs.get(tileId);
    if (existingDiff && typeof existingDiff === 'object') {
      return;
    }
    if (this.snapshotFetches.has(tileId)) {
      return;
    }
    const authToken = this.managerToken;
    const managerUrl = this.managerUrl;
    if (!authToken || authToken.length === 0 || managerUrl.length === 0) {
      return;
    }
    const promise = fetchSessionStateSnapshot(sessionId, authToken, managerUrl)
      .then((diff) => {
        this.setCachedDiff(tileId, diff ?? null);
      })
      .catch((error) => {
        if (typeof window === 'undefined') {
          return;
        }
        let message: string;
        if (error instanceof Error) {
          message = error.message;
        } else if (typeof error === 'string') {
          message = error;
        } else {
          try {
            message = JSON.stringify(error);
          } catch {
            message = String(error);
          }
        }
        console.warn('[tile-controller] snapshot fetch failed', {
          tileId,
          sessionId,
          error: message,
        });
      })
      .finally(() => {
        this.snapshotFetches.delete(tileId);
      });
    this.snapshotFetches.set(tileId, promise);
  }

  private schedulePersist() {
    if (!this.persistCallback) {
      return;
    }
    if (this.persistTimer) {
      clearTimeout(this.persistTimer);
    }
    const layout = this.layout;
    this.persistTimer = setTimeout(() => {
      this.persistTimer = null;
      emitTelemetry('canvas.layout.persist', {
        beachId: this.privateBeachId ?? undefined,
        tileCount: Object.keys(layout.tiles).length,
        groupCount: Object.keys(layout.groups).length,
        agentCount: Object.keys(layout.agents).length,
      });
      this.persistCallback?.(layout as ApiCanvasLayout);
    }, PERSIST_DELAY_MS);
  }

  private emit() {
    for (const listener of this.subscribers) {
      try {
        listener();
      } catch (error) {
        console.warn('[tile-controller] subscriber error', error);
      }
    }
  }

  private emitTile(tileId: string) {
    const listeners = this.tileSubscribers.get(tileId);
    if (!listeners) return;
    for (const listener of listeners) {
      try {
        listener();
      } catch (error) {
        console.warn('[tile-controller] tile-listener error', { tileId, error });
      }
    }
  }

  private updateTileSnapshot(tileId: string, mutator: (snapshot: TileSnapshot) => TileSnapshot) {
    const current = this.getTileSnapshot(tileId);
    const next = mutator(current);
    this.tileSnapshots.set(tileId, next);
    this.emitTile(tileId);
  }

  private applyGridSnapshotToLayout(base: SharedCanvasLayout, snapshot: GridLayoutSnapshot): SharedCanvasLayout {
    let next = withLayoutDashboardMetadata(base, snapshot);
    const entries = Object.entries(snapshot.tiles);
    if (entries.length === 0) {
      return next;
    }
    const defaultGridCols = typeof snapshot.gridCols === 'number' ? snapshot.gridCols : undefined;
    const defaultRowHeight = typeof snapshot.rowHeightPx === 'number' ? snapshot.rowHeightPx : undefined;
    const defaultLayoutVersion = typeof snapshot.layoutVersion === 'number' ? snapshot.layoutVersion : undefined;
    const tiles: SharedCanvasLayout['tiles'] = { ...next.tiles };
    for (const [tileId, tile] of Object.entries(tiles)) {
      if (!tile.position) {
        tiles[tileId] = {
          ...tile,
          position: { x: 0, y: 0 },
        } as CanvasTileNode;
      }
      if (!tile.size) {
        tiles[tileId] = {
          ...tiles[tileId],
          size: { width: DEFAULT_TILE_WIDTH, height: DEFAULT_TILE_HEIGHT },
        } as CanvasTileNode;
      }
    }
    let mutated = false;
    for (const [tileId, metadata] of entries) {
      const existing =
        tiles[tileId] ??
        ({
          id: tileId,
          kind: 'application',
          position: { x: 0, y: 0 },
          size: { width: DEFAULT_TILE_WIDTH, height: DEFAULT_TILE_HEIGHT },
          zIndex: 1,
          metadata: {},
        } as CanvasTileNode);
      const normalized: TileGridMetadataUpdate = {
        ...metadata,
        layout: metadata.layout ? { ...metadata.layout } : metadata.layout,
      };
      if (normalized.gridCols === undefined && defaultGridCols !== undefined) {
        normalized.gridCols = defaultGridCols;
      }
      if (normalized.rowHeightPx === undefined && defaultRowHeight !== undefined) {
        normalized.rowHeightPx = defaultRowHeight;
      }
      if (normalized.layoutVersion === undefined && defaultLayoutVersion !== undefined) {
        normalized.layoutVersion = defaultLayoutVersion;
      }
      const updated = withTileGridMetadata(existing, normalized);
      if (updated !== existing || !tiles[tileId]) {
        mutated = true;
        tiles[tileId] = updated;
      }
    }
    if (!mutated) {
      return next;
    }
    return {
      ...next,
      tiles,
    };
  }
}

function shallowViewerEqual(a: TerminalViewerState, b: TerminalViewerState) {
  return (
    a.store === b.store &&
    a.transport === b.transport &&
    a.connecting === b.connecting &&
    a.error === b.error &&
    a.status === b.status &&
    a.secureSummary === b.secureSummary &&
    a.latencyMs === b.latencyMs &&
    (a as any).transportVersion === (b as any).transportVersion
  );
}

export const sessionTileController = new SessionTileController();

if (typeof window !== 'undefined' && process.env.NODE_ENV !== 'production') {
  const globalWindow = window as typeof window & {
    __PRIVATE_BEACH_TILE_CONTROLLER__?: SessionTileController;
  };
  if (!globalWindow.__PRIVATE_BEACH_TILE_CONTROLLER__) {
    globalWindow.__PRIVATE_BEACH_TILE_CONTROLLER__ = sessionTileController;
  }
}

export function useCanvasSnapshot(): ControllerSnapshot {
  return useSyncExternalStore(
    (listener) => sessionTileController.subscribe(listener),
    () => sessionTileController.getSnapshot(),
    () => sessionTileController.getSnapshot(),
  );
}

export function useTileSnapshot(tileId: string): TileSnapshot {
  return useSyncExternalStore(
    (listener) => sessionTileController.subscribeTile(tileId, listener),
    () => sessionTileController.getTileSnapshot(tileId),
    () => sessionTileController.getTileSnapshot(tileId),
  );
}
