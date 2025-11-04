'use client';

import { createContext, useContext, useMemo, useReducer } from 'react';
import {
  DEFAULT_TILE_HEIGHT,
  DEFAULT_TILE_WIDTH,
} from './constants';
import type {
  TileCreateInput,
  TileDescriptor,
  TilePosition,
  TileResizeInput,
  TileSessionMeta,
  TileState,
} from './types';
import { computeAutoPosition, generateTileId, snapPosition, snapSize } from './utils';

type Action =
  | { type: 'UPSERT_TILE'; payload: { input: TileCreateInput } }
  | { type: 'REMOVE_TILE'; payload: { id: string } }
  | { type: 'SET_ACTIVE_TILE'; payload: { id: string | null } }
  | { type: 'BRING_TO_FRONT'; payload: { id: string } }
  | { type: 'SET_TILE_POSITION'; payload: { id: string; position: Partial<TilePosition> } }
  | { type: 'SET_TILE_SIZE'; payload: { id: string; size: TileResizeInput } }
  | { type: 'START_RESIZE'; payload: { id: string } }
  | { type: 'END_RESIZE'; payload: { id: string } };

const INITIAL_STATE: TileState = {
  tiles: {},
  order: [],
  activeId: null,
  resizing: {},
};

function ensureOrder(order: string[], id: string): string[] {
  return order.includes(id) ? order : [...order, id];
}

function bringToFront(order: string[], id: string): string[] {
  const filtered = order.filter((value) => value !== id);
  return [...filtered, id];
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
      const descriptor: TileDescriptor = existing
        ? {
            ...existing,
            nodeType: input.nodeType ?? existing.nodeType,
            position,
            size,
            sessionMeta,
            updatedAt: now,
          }
        : {
            id,
            nodeType: input.nodeType ?? 'application',
            position,
            size,
            sessionMeta,
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
      return {
        tiles,
        order,
        activeId: shouldFocus ? id : state.activeId,
        resizing,
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
      return { tiles, order, activeId, resizing };
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
    default:
      return state;
  }
}

const TileStateContext = createContext<TileState>(INITIAL_STATE);
const TileDispatchContext = createContext<React.Dispatch<Action>>(() => {
  throw new Error('TileDispatchContext not initialized');
});

export function TileStoreProvider({ children }: { children: React.ReactNode }) {
  const [state, dispatch] = useReducer(reducer, INITIAL_STATE);
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
      resizeTile: (id: string, size: TileResizeInput) =>
        dispatch({ type: 'SET_TILE_SIZE', payload: { id, size } }),
      beginResize: (id: string) => dispatch({ type: 'START_RESIZE', payload: { id } }),
      endResize: (id: string) => dispatch({ type: 'END_RESIZE', payload: { id } }),
    }),
    [dispatch],
  );
}
