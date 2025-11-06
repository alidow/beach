import type { CanvasLayout } from '@/lib/api';
import { DEFAULT_TILE_HEIGHT, DEFAULT_TILE_WIDTH } from './constants';
import type { TileDescriptor, TileSessionMeta, TileState } from './types';

const DEFAULT_VIEWPORT = { zoom: 1, pan: { x: 0, y: 0 } } as const;

function buildEmptyLayout(): CanvasLayout {
  const now = Date.now();
  return {
    version: 3,
    viewport: { ...DEFAULT_VIEWPORT },
    tiles: {},
    agents: {},
    groups: {},
    controlAssignments: {},
    metadata: { createdAt: now, updatedAt: now },
  };
}

function normalizeSessionMeta(input: unknown): TileSessionMeta | null {
  if (!input || typeof input !== 'object') {
    return null;
  }
  const record = input as Record<string, unknown>;
  const candidate = record.sessionMeta ?? record.session_meta ?? null;
  if (!candidate || typeof candidate !== 'object') {
    return null;
  }
  const source = candidate as Record<string, unknown>;
  const sessionId = typeof source.sessionId === 'string' ? source.sessionId : undefined;
  const title = typeof source.title === 'string' ? source.title : undefined;
  const status = typeof source.status === 'string' ? source.status : undefined;
  const harnessType = typeof source.harnessType === 'string' ? source.harnessType : undefined;
  const pendingActionsRaw = source.pendingActions;
  const pendingActions =
    typeof pendingActionsRaw === 'number' && Number.isFinite(pendingActionsRaw)
      ? pendingActionsRaw
      : undefined;

  if (!sessionId && !title && !status && !harnessType && pendingActions === undefined) {
    return null;
  }
  return {
    sessionId,
    title: title ?? null,
    status: status ?? null,
    harnessType: harnessType ?? null,
    pendingActions: pendingActions ?? null,
  };
}

function extractTileDescriptor(
  tileId: string,
  tile: CanvasLayout['tiles'][string],
  fallbackTimestamp: number,
): TileDescriptor {
  const now = Date.now();
  const position = {
    x: tile?.position?.x ?? 0,
    y: tile?.position?.y ?? 0,
  };
  const size = {
    width: tile?.size?.width ?? DEFAULT_TILE_WIDTH,
    height: tile?.size?.height ?? DEFAULT_TILE_HEIGHT,
  };
  const metadataRecord =
    tile?.metadata && typeof tile.metadata === 'object'
      ? (tile.metadata as Record<string, unknown>)
      : undefined;
  const createdAt =
    typeof metadataRecord?.createdAt === 'number'
      ? (metadataRecord.createdAt as number)
      : fallbackTimestamp;
  const updatedAt =
    typeof metadataRecord?.updatedAt === 'number'
      ? (metadataRecord.updatedAt as number)
      : now;

  return {
    id: tile?.id ?? tileId,
    nodeType: 'application',
    position,
    size,
    sessionMeta: normalizeSessionMeta(metadataRecord),
    createdAt,
    updatedAt,
  };
}

export function layoutToTileState(layout: CanvasLayout | null | undefined): TileState {
  const base = layout ?? buildEmptyLayout();
  const timestamp = base.metadata?.updatedAt ?? base.metadata?.createdAt ?? Date.now();
  const entries = Object.entries(base.tiles ?? {}).sort(([, a], [, b]) => {
    const aZ = a?.zIndex ?? 0;
    const bZ = b?.zIndex ?? 0;
    if (aZ === bZ) {
      const aId = a?.id ?? '';
      const bId = b?.id ?? '';
      return aId.localeCompare(bId);
    }
    return aZ - bZ;
  });

  const tiles: Record<string, TileDescriptor> = {};
  const order: string[] = [];
  let interactiveId: string | null = null;
  for (const [tileKey, tile] of entries) {
    const descriptor = extractTileDescriptor(tileKey, tile, timestamp);
    tiles[descriptor.id] = descriptor;
    order.push(descriptor.id);
    if (!interactiveId && !descriptor.sessionMeta?.sessionId) {
      interactiveId = descriptor.id;
    }
  }

  return {
    tiles,
    order,
    activeId: null,
    resizing: {},
    interactiveId,
  };
}

export function tileStateToLayout(state: TileState, baseLayout?: CanvasLayout | null): CanvasLayout {
  const base = baseLayout ?? buildEmptyLayout();
  const now = Date.now();
  const tiles: CanvasLayout['tiles'] = {};
  const baseTiles = base.tiles ?? {};
  const canonicalOrder = buildCanonicalOrder(state);

  canonicalOrder.forEach((tileId, orderIndex) => {
    const tile = state.tiles[tileId];
    if (!tile) {
      return;
    }
    const previous = baseTiles[tile.id] ?? baseTiles[tileId];
    const metadataBase: Record<string, unknown> =
      previous && previous.metadata && typeof previous.metadata === 'object'
        ? { ...(previous.metadata as Record<string, unknown>) }
        : {};
    if (tile.sessionMeta) {
      metadataBase.sessionMeta = tile.sessionMeta;
    } else if ('sessionMeta' in metadataBase) {
      delete metadataBase.sessionMeta;
    }

    tiles[tile.id] = {
      id: tile.id,
      kind: 'application',
      position: { ...tile.position },
      size: { ...tile.size },
      zIndex: orderIndex + 1,
      groupId: previous?.groupId,
      zoom: previous?.zoom,
      locked: previous?.locked,
      toolbarPinned: previous?.toolbarPinned,
      metadata: Object.keys(metadataBase).length > 0 ? metadataBase : undefined,
    };
  });

  return {
    version: 3,
    viewport: base.viewport ?? { ...DEFAULT_VIEWPORT },
    tiles,
    agents: base.agents ?? {},
    groups: base.groups ?? {},
    controlAssignments: base.controlAssignments ?? {},
    metadata: {
      createdAt: base.metadata?.createdAt ?? now,
      updatedAt: now,
      migratedFrom: base.metadata?.migratedFrom,
    },
  };
}

export function serializeTileStateKey(state: TileState): string {
  const order = buildCanonicalOrder(state);
  if (order.length === 0) {
    return 'tiles:none';
  }
  return order
    .map((tileId) => {
      const tile = state.tiles[tileId];
      if (!tile) {
        return `${tileId}:missing`;
      }
      const meta = tile.sessionMeta
        ? [
            tile.sessionMeta.sessionId ?? '',
            tile.sessionMeta.title ?? '',
            tile.sessionMeta.status ?? '',
            tile.sessionMeta.harnessType ?? '',
            tile.sessionMeta.pendingActions ?? '',
          ].join('~')
        : 'meta:none';
      return [
        tile.id,
        tile.position.x,
        tile.position.y,
        tile.size.width,
        tile.size.height,
        meta,
      ].join(':');
    })
    .join('|');
}

function buildCanonicalOrder(state: TileState): string[] {
  const seen = new Set<string>();
  const order: string[] = [];
  for (const id of state.order) {
    if (id && !seen.has(id)) {
      order.push(id);
      seen.add(id);
    }
  }
  for (const id of Object.keys(state.tiles)) {
    if (!seen.has(id)) {
      order.push(id);
      seen.add(id);
    }
  }
  return order;
}
