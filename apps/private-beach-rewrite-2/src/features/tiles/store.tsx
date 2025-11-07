'use client';

import { createContext, useContext, useMemo, useReducer } from 'react';
import { DEFAULT_TILE_HEIGHT, DEFAULT_TILE_WIDTH } from './constants';
import type {
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
  | { type: 'SET_TILE_VIEWPORT'; payload: { id: string; viewport: TileViewportSnapshot | null } };

function createEmptyState(): TileState {
  return {
    tiles: {},
    order: [],
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

function ensureAgentMeta(
  desiredType: TileNodeType,
  existingMeta: TileDescriptor['agentMeta'],
  inputMeta?: TileCreateInput['agentMeta'],
): TileDescriptor['agentMeta'] {
  if (inputMeta !== undefined) {
    return inputMeta;
  }
  if (existingMeta) {
    return existingMeta;
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
      return {
        ...state,
        tiles,
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
      const activeId =
        state.activeId === id ? (order.length > 0 ? order[order.length - 1] : null) : state.activeId;
      const interactiveId = state.interactiveId === id ? null : state.interactiveId;
      const viewport = { ...state.viewport };
      delete viewport[id];
      return { ...state, tiles, order, activeId, resizing, interactiveId, viewport };
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
    }),
    [dispatch],
  );
}
