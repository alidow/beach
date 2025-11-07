'use client';

// Shared primitive types
export type CanvasViewport = {
  x: number;
  y: number;
  zoom: number; // 1 = 100%
};

export type CanvasLayoutViewport = {
  zoom: number;
  pan: { x: number; y: number };
};

export type CanvasPoint = { x: number; y: number };

export type CanvasEdge = {
  id: string;
  source: string;
  target: string;
  type?: 'assignment' | 'group' | string;
  data?: Record<string, unknown>;
};

export type CanvasAgentRelationship = {
  id: string;
  sourceId: string;
  targetId: string;
  sourceHandleId?: string | null;
  targetHandleId?: string | null;
  instructions?: string | null;
  updateMode?: 'idle-summary' | 'push' | 'poll';
  pollFrequency?: number | null;
};

// Node types
export type CanvasTileNode = {
  id: string;
  kind?: 'application';
  position: { x: number; y: number };
  size: { width: number; height: number };
  zIndex: number;
  groupId?: string;
  metadata?: Record<string, unknown>;
};

export type CanvasAgentNode = {
  id: string;
  position: { x: number; y: number };
  size: { width: number; height: number };
  zIndex: number;
  metadata?: Record<string, unknown>;
};

export type CanvasGroupNode = {
  id: string;
  name?: string;
  memberIds: string[];
  position: { x: number; y: number };
  size: { width: number; height: number };
  zIndex: number;
  padding?: number;
  metadata?: Record<string, unknown>;
};

export type DropTarget =
  | { type: 'none' }
  | { type: 'tile'; id: string }
  | { type: 'group'; id: string }
  | { type: 'agent'; id: string };

// Canvas layout v3 contract (frontend perspective)
export type CanvasLayout = {
  version: 3;
  tiles: Record<string, CanvasTileNode>;
  groups: Record<string, CanvasGroupNode>;
  agents: Record<string, CanvasAgentNode>;
  edges?: CanvasEdge[];
  viewport?: CanvasLayoutViewport;
  metadata: {
    createdAt: number;
    updatedAt: number;
    migratedFrom?: number;
    agentRelationships?: Record<string, CanvasAgentRelationship>;
    agentRelationshipOrder?: string[];
  } & Record<string, unknown>;
  controlAssignments: Record<string, { controllerId: string; targetType: 'tile' | 'group'; targetId: string }>;
};

// Transitional helper (internal) for the surface component mapping
export type CanvasNodeType = 'tile' | 'agent' | 'group' | string;

export type CanvasNodeBase = {
  id: string;
  type: CanvasNodeType;
  xPx: number;
  yPx: number;
  widthPx: number;
  heightPx: number;
  zIndex?: number;
  parentId?: string | null;
  data?: Record<string, unknown>;
};

export type CanvasLayoutV3 = {
  version: 3;
  nodes: CanvasNodeBase[];
  edges: CanvasEdge[];
  viewport?: CanvasViewport;
  updatedAt?: number;
  metadata?: Record<string, unknown>;
};

// Lightweight adapter shape for incoming legacy grid items when needed
export type LegacyLayoutItem = {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  widthPx?: number;
  heightPx?: number;
  zoom?: number;
  locked?: boolean;
  toolbarPinned?: boolean;
};
