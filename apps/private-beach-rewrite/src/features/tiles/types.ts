'use client';

export type TileNodeType = 'application';

export type TilePosition = {
  x: number;
  y: number;
};

export type TileSize = {
  width: number;
  height: number;
};

export type TileSessionMeta = {
  sessionId?: string;
  title?: string | null;
  status?: string | null;
  harnessType?: string | null;
  pendingActions?: number | null;
};

export type TileDescriptor = {
  id: string;
  nodeType: TileNodeType;
  position: TilePosition;
  size: TileSize;
  sessionMeta?: TileSessionMeta | null;
  createdAt: number;
  updatedAt: number;
};

export type TileState = {
  tiles: Record<string, TileDescriptor>;
  order: string[];
  activeId: string | null;
  resizing: Record<string, boolean>;
};

export type TileCreateInput = {
  id?: string;
  nodeType?: TileNodeType;
  position?: Partial<TilePosition>;
  size?: Partial<TileSize>;
  sessionMeta?: TileSessionMeta | null;
  focus?: boolean;
};

export type TileResizeInput = {
  width: number;
  height: number;
};
