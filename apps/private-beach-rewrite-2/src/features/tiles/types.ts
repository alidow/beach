'use client';

export type TileNodeType = 'application' | 'agent';

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

export type AgentTraceMetadata = {
  enabled: boolean;
  trace_id?: string | null;
};

export type AgentMetadata = {
  role: string;
  responsibility: string;
  isEditing: boolean;
  trace?: AgentTraceMetadata;
};

export type RelationshipDescriptor = {
  id: string;
  sourceId: string;
  targetId: string;
  sourceSessionId?: string | null;
  targetSessionId?: string | null;
  sourceHandleId?: string | null;
  targetHandleId?: string | null;
  instructions: string;
  updateMode: 'idle-summary' | 'push' | 'poll';
  pollFrequency: number;
};

export type TileDescriptor = {
  id: string;
  nodeType: TileNodeType;
  position: TilePosition;
  size: TileSize;
  sessionMeta?: TileSessionMeta | null;
  agentMeta?: AgentMetadata | null;
  createdAt: number;
  updatedAt: number;
};

export type TileViewportSnapshot = {
  tileId: string;
  hostRows: number | null;
  hostCols: number | null;
  viewportRows: number | null;
  viewportCols: number | null;
  pixelsPerRow: number | null;
  pixelsPerCol: number | null;
};

export type TileState = {
  tiles: Record<string, TileDescriptor>;
  order: string[];
  relationships: Record<string, RelationshipDescriptor>;
  relationshipOrder: string[];
  activeId: string | null;
  resizing: Record<string, boolean>;
  interactiveId: string | null;
  viewport: Record<string, TileViewportSnapshot>;
};

export type TileCreateInput = {
  id?: string;
  nodeType?: TileNodeType;
  position?: Partial<TilePosition>;
  size?: Partial<TileSize>;
  sessionMeta?: TileSessionMeta | null;
  agentMeta?: AgentMetadata | null;
  focus?: boolean;
};

export type TileResizeInput = {
  width: number;
  height: number;
};
