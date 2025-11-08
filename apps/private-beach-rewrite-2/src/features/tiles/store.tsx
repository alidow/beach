'use client';

import { createContext, useContext, useMemo, useReducer } from 'react';
import { DEFAULT_TILE_HEIGHT, DEFAULT_TILE_WIDTH } from './constants';
import type {
  AgentMetadata,
  AgentTraceMetadata,
  RelationshipDescriptor,
  TileCreateInput,
  TileDescriptor,
  TileNodeType,
  TilePosition,
  TileResizeInput,
  TileSessionMeta,
  TileState,
  TileViewportSnapshot,
} from './types';
import { computeAutoPosition, generateTileId, snapPosition, snapSize } from './utils';

type Action =
  | { type: 'UPSERT_TILE'; payload: { input: TileCreateInput } }
  | { type: 'REMOVE_TILE'; payload: { id: string } }
  | { type: 'SET_ACTIVE_TILE'; payload: { id: string | null } }
  | { type: 'BRING_TO_FRONT'; payload: { id: string } }
  | { type: 'SET_TILE_POSITION'; payload: { id: string; position: Partial<TilePosition> } }
  | { type: 'SET_TILE_POSITION_IMMEDIATE'; payload: { id: string; position: Partial<TilePosition> } }
  | { type: 'SET_TILE_SIZE'; payload: { id: string; size: TileResizeInput } }
  | { type: 'START_RESIZE'; payload: { id: string } }
  | { type: 'END_RESIZE'; payload: { id: string } }
  | { type: 'SET_INTERACTIVE_TILE'; payload: { id: string | null } }
  | { type: 'SET_TILE_VIEWPORT'; payload: { id: string; viewport: TileViewportSnapshot | null } }
  | {
      type: 'ADD_RELATIONSHIP';
      payload: {
        id: string;
        sourceId: string;
        targetId: string;
        sourceHandleId?: string | null;
        targetHandleId?: string | null;
      };
    }
  | {
      type: 'UPDATE_RELATIONSHIP';
      payload: {
        id: string;
        patch: Partial<Omit<TileState['relationships'][string], 'id' | 'sourceId' | 'targetId'>>;
      };
    }
  | { type: 'REMOVE_RELATIONSHIP'; payload: { id: string } };

function createEmptyState(): TileState {
  return {
    tiles: {},
    order: [],
    relationships: {},
    relationshipOrder: [],
    activeId: null,
    resizing: {},
    interactiveId: null,
    viewport: {},
  };
}

function ensureOrder(order: string[], id: string): string[] {
  return order.includes(id) ? order : [...order, id];
}

function bringToFront(order: string[], id: string): string[] {
  const filtered = order.filter((value) => value !== id);
  return [...filtered, id];
}

type LegacyAgentMeta = AgentMetadata & {
  traceEnabled?: boolean;
  traceId?: string | null;
};

function coerceAgentTrace(meta: LegacyAgentMeta | TileCreateInput['agentMeta'] | null | undefined): AgentTraceMetadata | undefined {
  if (!meta) {
    return undefined;
  }
  const record = meta as Record<string, unknown>;
  const rawTrace = record.trace;
  if (rawTrace && typeof rawTrace === 'object' && rawTrace !== null) {
    const enabled = typeof (rawTrace as Record<string, unknown>).enabled === 'boolean' ? Boolean((rawTrace as Record<string, unknown>).enabled) : undefined;
    if (typeof enabled === 'boolean') {
      const normalized: AgentTraceMetadata = { enabled };
      const traceId = (rawTrace as Record<string, unknown>).trace_id;
      if (typeof traceId === 'string' && traceId.trim().length > 0) {
        normalized.trace_id = traceId.trim();
      }
      return normalized;
    }
  }
  if (typeof record.traceEnabled === 'boolean') {
    const normalized: AgentTraceMetadata = { enabled: Boolean(record.traceEnabled) };
    if (record.traceEnabled && typeof record.traceId === 'string' && record.traceId.trim().length > 0) {
      normalized.trace_id = record.traceId.trim();
    }
    return normalized;
  }
  return undefined;
}

function ensureAgentMeta(
  desiredType: TileNodeType,
  existingMeta: TileDescriptor['agentMeta'],
  inputMeta?: TileCreateInput['agentMeta'],
): TileDescriptor['agentMeta'] {
  if (inputMeta !== undefined) {
    const trace = coerceAgentTrace(inputMeta);
    const meta: AgentMetadata = {
      role: inputMeta.role ?? '',
      responsibility: inputMeta.responsibility ?? '',
      isEditing: typeof inputMeta.isEditing === 'boolean' ? inputMeta.isEditing : true,
    };
    if (trace) {
      meta.trace = trace;
    }
    return meta;
  }
  if (existingMeta) {
    const trace = coerceAgentTrace(existingMeta as LegacyAgentMeta);
    const meta: AgentMetadata = {
      role: existingMeta.role,
      responsibility: existingMeta.responsibility,
      isEditing: existingMeta.isEditing,
    };
    if (trace) {
      meta.trace = trace;
    }
    return meta;
  }
  if (desiredType === 'agent') {
    return { role: '', responsibility: '', isEditing: true };
  }
  return null;
}

function viewportEqual(a: TileViewportSnapshot | undefined, b: TileViewportSnapshot | null): boolean {
  if (!a && !b) {
    return true;
  }
  if (!a || !b) {
    return false;
  }
  return (
    a.hostRows === b.hostRows &&
    a.hostCols === b.hostCols &&
    a.viewportRows === b.viewportRows &&
    a.viewportCols === b.viewportCols &&
    a.pixelsPerRow === b.pixelsPerRow &&
    a.pixelsPerCol === b.pixelsPerCol
  );
}

function syncRelationshipSessionRefs(
  relationships: Record<string, RelationshipDescriptor>,
  tile: TileDescriptor | undefined,
): Record<string, RelationshipDescriptor> | null {
  if (!tile) {
    return null;
  }
  const sessionId = tile.sessionMeta?.sessionId ?? null;
  let next: Record<string, RelationshipDescriptor> | null = null;
  for (const [relId, rel] of Object.entries(relationships)) {
    const ensureNext = () => {
      if (!next) {
        next = { ...relationships };
      }
      if (!next[relId]) {
        next[relId] = { ...rel };
      }
      return next[relId];
    };
    if (rel.sourceId === tile.id && rel.sourceSessionId !== sessionId) {
      const target = ensureNext();
      target.sourceSessionId = sessionId;
    }
    if (rel.targetId === tile.id && rel.targetSessionId !== sessionId) {
      const target = ensureNext();
      target.targetSessionId = sessionId;
    }
  }
  return next;
}

function reducer(state: TileState, action: Action): TileState {
  switch (action.type) {
    case 'UPSERT_TILE': {
      const input = action.payload.input ?? {};
      const id = generateTileId(state, input.id);
      const existing = state.tiles[id];
      const now = Date.now();
      const baseSize = existing?.size ?? { width: DEFAULT_TILE_WIDTH, height: DEFAULT_TILE_HEIGHT };
      const size = snapSize({ ...baseSize, ...input.size });
      const basePosition = existing?.position ?? computeAutoPosition(state, size);
      const position = snapPosition({ ...basePosition, ...input.position });
      const sessionMeta =
        input.sessionMeta !== undefined ? input.sessionMeta : existing?.sessionMeta ?? null;
      const nodeType = input.nodeType ?? existing?.nodeType ?? 'application';
      const agentMeta = ensureAgentMeta(nodeType, existing?.agentMeta ?? null, input.agentMeta);
      const descriptor: TileDescriptor = existing
        ? {
            ...existing,
            nodeType,
            position,
            size,
            sessionMeta,
            agentMeta,
            updatedAt: now,
          }
        : {
            id,
            nodeType,
            position,
            size,
            sessionMeta,
            agentMeta,
            createdAt: now,
            updatedAt: now,
          };
      const tiles = { ...state.tiles, [id]: descriptor };
      let order = ensureOrder(state.order, id);
      const shouldFocus = input.focus ?? !existing;
      if (shouldFocus) {
        order = bringToFront(order, id);
      }
      const resizing = state.resizing[id]
        ? (() => {
            const next = { ...state.resizing };
            delete next[id];
            return next;
          })()
        : state.resizing;
      let interactiveId = state.interactiveId && !tiles[state.interactiveId] ? null : state.interactiveId;
      const hasSession = Boolean(descriptor.sessionMeta?.sessionId);
      const previouslyHadSession = Boolean(existing?.sessionMeta?.sessionId);
      if (!existing && !hasSession) {
        interactiveId = descriptor.id;
      } else if (previouslyHadSession && !hasSession) {
        interactiveId = descriptor.id;
      } else if (interactiveId === descriptor.id && hasSession && !previouslyHadSession && !shouldFocus) {
        // keep as-is unless it was auto-selected above; no-op
      }
      let nextRelationships = state.relationships;
      const synced = syncRelationshipSessionRefs(state.relationships, descriptor);
      if (synced) {
        nextRelationships = synced;
      }
      return {
        ...state,
        tiles,
        relationships: nextRelationships,
        order,
        activeId: shouldFocus ? id : state.activeId,
        resizing,
        interactiveId,
      };
    }
    case 'REMOVE_TILE': {
      const { id } = action.payload;
      if (!state.tiles[id]) {
        return state;
      }
      const tiles = { ...state.tiles };
      delete tiles[id];
      const order = state.order.filter((value) => value !== id);
      const resizing = { ...state.resizing };
      delete resizing[id];
      const relationships = { ...state.relationships };
      const relationshipOrder = state.relationshipOrder.filter((relId) => {
        const rel = relationships[relId];
        if (!rel) {
          return false;
        }
        if (rel.sourceId === id || rel.targetId === id) {
          delete relationships[relId];
          return false;
        }
        return true;
      });
      const activeId =
        state.activeId === id ? (order.length > 0 ? order[order.length - 1] : null) : state.activeId;
      const interactiveId = state.interactiveId === id ? null : state.interactiveId;
      const viewport = { ...state.viewport };
      delete viewport[id];
      return {
        ...state,
        tiles,
        order,
        relationships,
        relationshipOrder,
        activeId,
        resizing,
        interactiveId,
        viewport,
      };
    }
    case 'SET_ACTIVE_TILE': {
      const { id } = action.payload;
      if (id === null) {
        if (state.activeId === null) {
          return state;
        }
        return { ...state, activeId: null };
      }
      if (!state.tiles[id]) {
        return state;
      }
      const order = bringToFront(state.order, id);
      return { ...state, activeId: id, order };
    }
    case 'BRING_TO_FRONT': {
      const { id } = action.payload;
      if (!state.tiles[id]) {
        return state;
      }
      const order = bringToFront(state.order, id);
      return order === state.order ? state : { ...state, order };
    }
    case 'SET_TILE_POSITION': {
      const { id, position: patch } = action.payload;
      const tile = state.tiles[id];
      if (!tile) {
        return state;
      }
      const position = snapPosition({ ...tile.position, ...patch });
      if (position.x === tile.position.x && position.y === tile.position.y) {
        return state;
      }
      const tiles = {
        ...state.tiles,
        [id]: {
          ...tile,
          position,
          updatedAt: Date.now(),
        },
      };
      return { ...state, tiles };
    }
    case 'SET_TILE_POSITION_IMMEDIATE': {
      const { id, position: patch } = action.payload;
      const tile = state.tiles[id];
      if (!tile) {
        return state;
      }
      const x =
        typeof patch.x === 'number' && Number.isFinite(patch.x) ? patch.x : tile.position.x;
      const y =
        typeof patch.y === 'number' && Number.isFinite(patch.y) ? patch.y : tile.position.y;
      if (x === tile.position.x && y === tile.position.y) {
        return state;
      }
      const tiles = {
        ...state.tiles,
        [id]: {
          ...tile,
          position: { x, y },
          updatedAt: Date.now(),
        },
      };
      return { ...state, tiles };
    }
    case 'SET_TILE_SIZE': {
      const { id, size } = action.payload;
      const tile = state.tiles[id];
      if (!tile) {
        return state;
      }
      const nextSize = snapSize({ ...tile.size, ...size });
      if (nextSize.width === tile.size.width && nextSize.height === tile.size.height) {
        return state;
      }
      const tiles = {
        ...state.tiles,
        [id]: {
          ...tile,
          size: nextSize,
          updatedAt: Date.now(),
        },
      };
      return { ...state, tiles };
    }
    case 'START_RESIZE': {
      const { id } = action.payload;
      if (!state.tiles[id]) {
        return state;
      }
      const resizing = state.resizing[id]
        ? state.resizing
        : { ...state.resizing, [id]: true };
      const order = bringToFront(state.order, id);
      return {
        ...state,
        resizing,
        order,
        activeId: id,
      };
    }
    case 'END_RESIZE': {
      const { id } = action.payload;
      if (!state.resizing[id]) {
        return state;
      }
      const resizing = { ...state.resizing };
      delete resizing[id];
      return { ...state, resizing };
    }
    case 'SET_INTERACTIVE_TILE': {
      const { id } = action.payload;
      if (id === null) {
        if (state.interactiveId === null) {
          return state;
        }
        return { ...state, interactiveId: null };
      }
      if (!state.tiles[id]) {
        return state.interactiveId === null ? state : { ...state, interactiveId: null };
      }
      if (state.interactiveId === id) {
        return state;
      }
      return { ...state, interactiveId: id };
    }
    case 'SET_TILE_VIEWPORT': {
      const { id, viewport } = action.payload;
      if (!state.tiles[id]) {
        if (!state.viewport[id]) {
          return state;
        }
        const nextViewport = { ...state.viewport };
        delete nextViewport[id];
        return { ...state, viewport: nextViewport };
      }
      if (!viewport) {
        if (!state.viewport[id]) {
          return state;
        }
        const nextViewport = { ...state.viewport };
        delete nextViewport[id];
        return { ...state, viewport: nextViewport };
      }
      const existing = state.viewport[id];
      if (viewportEqual(existing, viewport)) {
        return state;
      }
      return {
        ...state,
        viewport: {
          ...state.viewport,
          [id]: viewport,
        },
      };
    }
    case 'ADD_RELATIONSHIP': {
      const { id, sourceId, targetId, sourceHandleId, targetHandleId } = action.payload;
      if (!state.tiles[sourceId] || !state.tiles[targetId]) {
        return state;
      }
      if (state.relationships[id]) {
        return state;
      }
      const sourceSessionId = state.tiles[sourceId]?.sessionMeta?.sessionId ?? null;
      const targetSessionId = state.tiles[targetId]?.sessionMeta?.sessionId ?? null;
      return {
        ...state,
        relationships: {
          ...state.relationships,
          [id]: {
            id,
            sourceId,
            targetId,
            sourceSessionId,
            targetSessionId,
            sourceHandleId: sourceHandleId ?? null,
            targetHandleId: targetHandleId ?? null,
            instructions: '',
            updateMode: 'idle-summary',
            pollFrequency: 60,
          },
        },
        relationshipOrder: state.relationshipOrder.includes(id)
          ? state.relationshipOrder
          : [...state.relationshipOrder, id],
      };
    }
    case 'UPDATE_RELATIONSHIP': {
      const { id, patch } = action.payload;
      const existing = state.relationships[id];
      if (!existing) {
        return state;
      }
      return {
        ...state,
        relationships: {
          ...state.relationships,
          [id]: {
            ...existing,
            ...patch,
          },
        },
      };
    }
    case 'REMOVE_RELATIONSHIP': {
      const { id } = action.payload;
      if (!state.relationships[id]) {
        return state;
      }
      const next = { ...state.relationships };
      delete next[id];
      return {
        ...state,
        relationships: next,
        relationshipOrder: state.relationshipOrder.filter((value) => value !== id),
      };
    }
    default:
      return state;
  }
}

function hydrateInitialState(state: TileState | null | undefined): TileState {
  const base = state ?? createEmptyState();
  return {
    ...createEmptyState(),
    ...base,
    tiles: base.tiles ?? {},
    order: Array.isArray(base.order) ? base.order : [],
    relationships: base.relationships ?? {},
    relationshipOrder: Array.isArray(base.relationshipOrder) ? base.relationshipOrder : [],
    resizing: base.resizing ?? {},
    interactiveId: base.interactiveId ?? null,
    viewport: base.viewport ?? {},
  };
}

const TileStateContext = createContext<TileState>(createEmptyState());
const TileDispatchContext = createContext<React.Dispatch<Action>>(() => {
  throw new Error('TileDispatchContext not initialized');
});

type TileStoreProviderProps = {
  children: React.ReactNode;
  initialState?: TileState;
};

export function TileStoreProvider({ children, initialState }: TileStoreProviderProps) {
  const [state, dispatch] = useReducer(reducer, initialState ?? null, hydrateInitialState);
  const memoState = useMemo(() => state, [state]);
  return (
    <TileDispatchContext.Provider value={dispatch}>
      <TileStateContext.Provider value={memoState}>{children}</TileStateContext.Provider>
    </TileDispatchContext.Provider>
  );
}

export function useTileState(): TileState {
  return useContext(TileStateContext);
}

export function useTileActions() {
  const dispatch = useContext(TileDispatchContext);
  return useMemo(
    () => ({
      createTile: (input: TileCreateInput = {}) => dispatch({ type: 'UPSERT_TILE', payload: { input } }),
      removeTile: (id: string) => dispatch({ type: 'REMOVE_TILE', payload: { id } }),
      setActiveTile: (id: string | null) => dispatch({ type: 'SET_ACTIVE_TILE', payload: { id } }),
      bringToFront: (id: string) => dispatch({ type: 'BRING_TO_FRONT', payload: { id } }),
      updateTileMeta: (id: string, sessionMeta: TileSessionMeta | null) =>
        dispatch({ type: 'UPSERT_TILE', payload: { input: { id, sessionMeta, focus: false } } }),
      setTilePosition: (id: string, position: Partial<TilePosition>) =>
        dispatch({ type: 'SET_TILE_POSITION', payload: { id, position } }),
      setTilePositionImmediate: (id: string, position: Partial<TilePosition>) =>
        dispatch({ type: 'SET_TILE_POSITION_IMMEDIATE', payload: { id, position } }),
      resizeTile: (id: string, size: TileResizeInput) =>
        dispatch({ type: 'SET_TILE_SIZE', payload: { id, size } }),
      beginResize: (id: string) => dispatch({ type: 'START_RESIZE', payload: { id } }),
      endResize: (id: string) => dispatch({ type: 'END_RESIZE', payload: { id } }),
      setInteractiveTile: (id: string | null) => dispatch({ type: 'SET_INTERACTIVE_TILE', payload: { id } }),
      updateTileViewport: (id: string, viewport: TileViewportSnapshot | null) =>
        dispatch({ type: 'SET_TILE_VIEWPORT', payload: { id, viewport } }),
      addRelationship: (
        id: string,
        sourceId: string,
        targetId: string,
        options?: { sourceHandleId?: string | null; targetHandleId?: string | null },
      ) =>
        dispatch({
          type: 'ADD_RELATIONSHIP',
          payload: { id, sourceId, targetId, sourceHandleId: options?.sourceHandleId, targetHandleId: options?.targetHandleId },
        }),
      updateRelationship: (
        id: string,
        patch: Partial<Omit<TileState['relationships'][string], 'id' | 'sourceId' | 'targetId'>>,
      ) => dispatch({ type: 'UPDATE_RELATIONSHIP', payload: { id, patch } }),
      removeRelationship: (id: string) => dispatch({ type: 'REMOVE_RELATIONSHIP', payload: { id } }),
    }),
    [dispatch],
  );
}
