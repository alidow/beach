import type { CanvasAgentRelationship, CanvasLayout } from '@/lib/api';
import { DEFAULT_TILE_HEIGHT, DEFAULT_TILE_WIDTH } from './constants';
import type {
  AgentMetadata,
  RelationshipDescriptor,
  TileDescriptor,
  TileSessionMeta,
  TileState,
} from './types';

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

function normalizeTraceMetadata(record: Record<string, unknown>): AgentMetadata['trace'] | undefined {
  const rawTrace = record.trace;
  if (rawTrace && typeof rawTrace === 'object') {
    const traceRecord = rawTrace as Record<string, unknown>;
    if (typeof traceRecord.enabled === 'boolean') {
      const normalized: AgentMetadata['trace'] = { enabled: traceRecord.enabled };
      const traceId = traceRecord.trace_id;
      if (typeof traceId === 'string' && traceId.trim().length > 0) {
        normalized.trace_id = traceId.trim();
      }
      return normalized;
    }
  }
  if (typeof record.traceEnabled === 'boolean') {
    const normalized: AgentMetadata['trace'] = { enabled: Boolean(record.traceEnabled) };
    const legacyTraceId = record.traceId;
    if (record.traceEnabled && typeof legacyTraceId === 'string' && legacyTraceId.trim().length > 0) {
      normalized.trace_id = legacyTraceId.trim();
    }
    return normalized;
  }
  return undefined;
}

function normalizeAgentMeta(source: unknown, nodeType: TileDescriptor['nodeType']): AgentMetadata | null {
  if (nodeType !== 'agent' || !source || typeof source !== 'object') {
    return null;
  }
  const record = source as Record<string, unknown>;
  const role = typeof record.role === 'string' ? record.role : '';
  const responsibility = typeof record.responsibility === 'string' ? record.responsibility : '';
  const isEditing =
    typeof record.isEditing === 'boolean' ? record.isEditing : !role && !responsibility;
  const agentMeta: AgentMetadata = {
    role,
    responsibility,
    isEditing,
  };
  const trace = normalizeTraceMetadata(record);
  if (trace) {
    agentMeta.trace = trace;
  }
  if (!role && !responsibility) {
    agentMeta.isEditing = true;
  }
  return agentMeta;
}

const VALID_RELATIONSHIP_MODES: RelationshipDescriptor['updateMode'][] = ['idle-summary', 'push', 'poll'];

function isRelationshipUpdateMode(value: unknown): value is RelationshipDescriptor['updateMode'] {
  return typeof value === 'string' && VALID_RELATIONSHIP_MODES.includes(value as RelationshipDescriptor['updateMode']);
}

function sanitizePollFrequency(value: unknown): number {
  if (typeof value === 'number' && Number.isFinite(value)) {
    const normalized = Math.round(value);
    return Math.max(5, Math.min(86400, normalized));
  }
  if (typeof value === 'string') {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) {
      const normalized = Math.round(parsed);
      return Math.max(5, Math.min(86400, normalized));
    }
  }
  return 60;
}

type SerializedRelationship = CanvasAgentRelationship;

function normalizeRelationshipEntry(input: unknown, fallbackId: string): RelationshipDescriptor | null {
  if (!input || typeof input !== 'object') {
    return null;
  }
  const record = input as Record<string, unknown>;
  const sourceId = typeof record.sourceId === 'string' ? record.sourceId : '';
  const targetId = typeof record.targetId === 'string' ? record.targetId : '';
  if (!sourceId || !targetId) {
    return null;
  }
  const idRaw = typeof record.id === 'string' && record.id.trim().length > 0 ? record.id : fallbackId;
  const updateMode = isRelationshipUpdateMode(record.updateMode) ? (record.updateMode as RelationshipDescriptor['updateMode']) : 'idle-summary';
  const pollFrequency = sanitizePollFrequency(record.pollFrequency);
  return {
    id: idRaw,
    sourceId,
    targetId,
    sourceSessionId: typeof record.sourceSessionId === 'string' ? record.sourceSessionId : null,
    targetSessionId: typeof record.targetSessionId === 'string' ? record.targetSessionId : null,
    sourceHandleId: typeof record.sourceHandleId === 'string' ? record.sourceHandleId : null,
    targetHandleId: typeof record.targetHandleId === 'string' ? record.targetHandleId : null,
    instructions: typeof record.instructions === 'string' ? record.instructions : '',
    updateMode,
    pollFrequency,
  };
}

function extractRelationshipsFromMetadata(
  layout: CanvasLayout,
  tiles: Record<string, TileDescriptor>,
): Pick<TileState, 'relationships' | 'relationshipOrder'> {
  const metadata = layout.metadata;
  const rawRelationships = metadata?.agentRelationships ?? {};
  const orderFromMetadata = Array.isArray(metadata?.agentRelationshipOrder)
    ? metadata?.agentRelationshipOrder.filter((value): value is string => typeof value === 'string' && value.length > 0)
    : [];
  const relationships: Record<string, RelationshipDescriptor> = {};
  const relationshipOrder: string[] = [];
  const pushRelationship = (descriptor: RelationshipDescriptor | null) => {
    if (!descriptor) {
      return;
    }
    if (!tiles[descriptor.sourceId] || !tiles[descriptor.targetId]) {
      return;
    }
    if (relationships[descriptor.id]) {
      return;
    }
    relationships[descriptor.id] = descriptor;
    relationshipOrder.push(descriptor.id);
  };
  for (const relId of orderFromMetadata) {
    pushRelationship(normalizeRelationshipEntry(rawRelationships?.[relId], relId));
  }
  for (const [relId, raw] of Object.entries(rawRelationships ?? {})) {
    if (relationships[relId]) {
      continue;
    }
    pushRelationship(normalizeRelationshipEntry(raw, relId));
  }
  for (const rel of Object.values(relationships)) {
    if (!rel) {
      continue;
    }
    const sourceSession = tiles[rel.sourceId]?.sessionMeta?.sessionId ?? null;
    const targetSession = tiles[rel.targetId]?.sessionMeta?.sessionId ?? null;
    rel.sourceSessionId = rel.sourceSessionId ?? sourceSession;
    rel.targetSessionId = rel.targetSessionId ?? targetSession;
  }
  return { relationships, relationshipOrder };
}

function serializeRelationships(state: TileState): {
  records: Record<string, SerializedRelationship> | null;
  order: string[];
} {
  const order = buildRelationshipOrder(state);
  if (order.length === 0) {
    return { records: null, order: [] };
  }
  const records: Record<string, SerializedRelationship> = {};
  for (const relId of order) {
    const rel = state.relationships[relId];
    if (!rel) {
      continue;
    }
    records[relId] = {
      id: rel.id,
      sourceId: rel.sourceId,
      targetId: rel.targetId,
      sourceSessionId: rel.sourceSessionId ?? null,
      targetSessionId: rel.targetSessionId ?? null,
      sourceHandleId: rel.sourceHandleId ?? null,
      targetHandleId: rel.targetHandleId ?? null,
      instructions: rel.instructions || null,
      updateMode: rel.updateMode,
      pollFrequency: rel.pollFrequency,
    };
  }
  return { records, order };
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
  const nodeTypeRaw =
    typeof metadataRecord?.nodeType === 'string'
      ? (metadataRecord.nodeType as string)
      : typeof tile?.kind === 'string'
        ? (tile.kind as string)
        : 'application';
  const nodeType = nodeTypeRaw === 'agent' ? 'agent' : 'application';
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
    nodeType,
    position,
    size,
    sessionMeta: normalizeSessionMeta(metadataRecord),
    agentMeta: normalizeAgentMeta(metadataRecord?.agentMeta, nodeType),
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
	for (const [tileKey, tile] of entries) {
		const descriptor = extractTileDescriptor(tileKey, tile, timestamp);
		tiles[descriptor.id] = descriptor;
		order.push(descriptor.id);
	}
	const { relationships, relationshipOrder } = extractRelationshipsFromMetadata(base, tiles);
	return {
		tiles,
		order,
		relationships,
		relationshipOrder,
		activeId: null,
		resizing: {},
		interactiveId: null,
		viewport: {},
	};
}

export function tileStateToLayout(state: TileState, baseLayout?: CanvasLayout | null): CanvasLayout {
	const base = baseLayout ?? buildEmptyLayout();
	const now = Date.now();
	const tilesOut: CanvasLayout['tiles'] = {};
	const baseTiles = base.tiles ?? {};
	const canonicalOrder = buildCanonicalOrder(state);
	const { records: serializedRelationships, order: serializedRelationshipOrder } = serializeRelationships(state);

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
    metadataBase.nodeType = tile.nodeType;
    if (tile.nodeType === 'agent') {
      metadataBase.agentMeta = tile.agentMeta ?? { role: '', responsibility: '', isEditing: true };
    } else if ('agentMeta' in metadataBase) {
      delete metadataBase.agentMeta;
    }

    tilesOut[tile.id] = {
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

	const metadataOut: CanvasLayout['metadata'] = {
		createdAt: base.metadata?.createdAt ?? now,
		updatedAt: now,
		migratedFrom: base.metadata?.migratedFrom,
	};
	if (serializedRelationships && Object.keys(serializedRelationships).length > 0) {
		metadataOut.agentRelationships = serializedRelationships;
		metadataOut.agentRelationshipOrder = serializedRelationshipOrder;
	}
	return {
		version: 3,
		viewport: base.viewport ?? { ...DEFAULT_VIEWPORT },
		tiles: tilesOut,
		agents: base.agents ?? {},
		groups: base.groups ?? {},
		controlAssignments: base.controlAssignments ?? {},
		metadata: metadataOut,
	};
}

export function serializeTileStateKey(state: TileState): string {
	const tileOrder = buildCanonicalOrder(state);
	const tileSignature =
		tileOrder.length === 0
			? 'tiles:none'
			: tileOrder
					.map((tileId) => {
						const tile = state.tiles[tileId];
						if (!tile) {
							return `${tileId}:missing`;
						}
					const sessionSignature = tile.sessionMeta
						? [
							 tile.sessionMeta.sessionId ?? '',
							 tile.sessionMeta.title ?? '',
					  ].join('~')
						: 'session:none';
						const agentSignature =
							tile.nodeType === 'agent'
								? [
									 tile.agentMeta?.role ?? '',
									 tile.agentMeta?.responsibility ?? '',
									 tile.agentMeta?.isEditing ? 'editing' : 'saved',
							  ].join('~')
								: 'agent:none';
						return [
							tile.id,
							tile.nodeType,
							tile.position.x,
							tile.position.y,
							tile.size.width,
							tile.size.height,
							sessionSignature,
							agentSignature,
						].join(':');
					})
					.join('|');
	const relationshipOrder = buildRelationshipOrder(state);
	const relationshipSignature =
		relationshipOrder.length === 0
			? 'relationships:none'
			: relationshipOrder
					.map((relId) => {
						const rel = state.relationships[relId];
						if (!rel) {
							return `${relId}:missing`;
						}
						return [
							rel.id,
							rel.sourceId,
							rel.targetId,
							rel.sourceHandleId ?? '',
							rel.targetHandleId ?? '',
							rel.instructions ?? '',
							rel.updateMode,
							rel.pollFrequency ?? '',
						].join(':');
					})
					.join('|');
	return `${tileSignature}::${relationshipSignature}`;
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

function buildRelationshipOrder(state: TileState): string[] {
	const seen = new Set<string>();
	const order: string[] = [];
	for (const id of state.relationshipOrder) {
		if (id && !seen.has(id) && state.relationships[id]) {
			order.push(id);
			seen.add(id);
		}
	}
	for (const id of Object.keys(state.relationships)) {
		if (!seen.has(id)) {
			order.push(id);
			seen.add(id);
		}
	}
	return order;
}
