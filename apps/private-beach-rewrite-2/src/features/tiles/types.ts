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
  transport?: 'webrtc' | 'http' | null;
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

export type RelationshipUpdateMode = 'idle-summary' | 'push' | 'poll' | 'hybrid';

export type RelationshipCadenceConfig = {
  idleSummary: boolean;
  allowChildPush: boolean;
  pollEnabled: boolean;
  pollFrequencySeconds: number;
  pollRequireContentChange: boolean;
  pollQuietWindowSeconds: number;
};

const DEFAULT_CADENCE_BASE: RelationshipCadenceConfig = {
  idleSummary: true,
  allowChildPush: true,
  pollEnabled: true,
  pollFrequencySeconds: 30,
  pollRequireContentChange: true,
  pollQuietWindowSeconds: 30,
};

const MIN_POLL_FREQUENCY = 1;
const MAX_POLL_FREQUENCY = 86400;

export function createDefaultRelationshipCadence(): RelationshipCadenceConfig {
  return { ...DEFAULT_CADENCE_BASE };
}

export function clampPollFrequency(seconds: number, fallback: number): number {
  if (!Number.isFinite(seconds)) {
    return fallback;
  }
  return Math.max(MIN_POLL_FREQUENCY, Math.min(MAX_POLL_FREQUENCY, Math.round(seconds)));
}

export function deriveCadenceFromLegacyMode(
  mode: RelationshipUpdateMode | undefined,
  pollFrequency: number,
): RelationshipCadenceConfig {
  const sanitizedFrequency = clampPollFrequency(pollFrequency, DEFAULT_CADENCE_BASE.pollFrequencySeconds);
  switch (mode) {
    case 'push':
      return {
        idleSummary: false,
        allowChildPush: true,
        pollEnabled: false,
        pollFrequencySeconds: sanitizedFrequency,
        pollRequireContentChange: false,
        pollQuietWindowSeconds: 0,
      };
    case 'poll':
      return {
        idleSummary: false,
        allowChildPush: false,
        pollEnabled: true,
        pollFrequencySeconds: sanitizedFrequency,
        pollRequireContentChange: false,
        pollQuietWindowSeconds: 0,
      };
    case 'idle-summary':
      return {
        idleSummary: true,
        allowChildPush: false,
        pollEnabled: false,
        pollFrequencySeconds: sanitizedFrequency,
        pollRequireContentChange: false,
        pollQuietWindowSeconds: 0,
      };
    case 'hybrid':
    default:
      return createDefaultRelationshipCadence();
  }
}

export function sanitizeRelationshipCadenceInput(
  input: unknown,
  legacyMode: RelationshipUpdateMode | undefined,
  legacyPollFrequency: number,
): RelationshipCadenceConfig {
  const fallback = deriveCadenceFromLegacyMode(legacyMode, legacyPollFrequency);
  if (!input || typeof input !== 'object') {
    return { ...fallback };
  }
  const record = input as Partial<RelationshipCadenceConfig>;
  const pollFrequencySeconds = clampPollFrequency(
    typeof record.pollFrequencySeconds === 'number' ? record.pollFrequencySeconds : fallback.pollFrequencySeconds,
    fallback.pollFrequencySeconds,
  );
  const quietSeconds =
    typeof record.pollQuietWindowSeconds === 'number' && Number.isFinite(record.pollQuietWindowSeconds)
      ? Math.max(0, Math.min(MAX_POLL_FREQUENCY, Math.round(record.pollQuietWindowSeconds)))
      : fallback.pollQuietWindowSeconds;
  return {
    idleSummary:
      typeof record.idleSummary === 'boolean' ? record.idleSummary : fallback.idleSummary,
    allowChildPush:
      typeof record.allowChildPush === 'boolean' ? record.allowChildPush : fallback.allowChildPush,
    pollEnabled: typeof record.pollEnabled === 'boolean' ? record.pollEnabled : fallback.pollEnabled,
    pollFrequencySeconds,
    pollRequireContentChange:
      typeof record.pollRequireContentChange === 'boolean'
        ? record.pollRequireContentChange
        : fallback.pollRequireContentChange,
    pollQuietWindowSeconds: quietSeconds,
  };
}

export function inferUpdateModeFromCadence(cadence: RelationshipCadenceConfig): RelationshipUpdateMode {
  const enabled = [
    cadence.idleSummary,
    cadence.allowChildPush,
    cadence.pollEnabled,
  ].filter(Boolean).length;
  if (enabled === 0) {
    return 'idle-summary';
  }
  if (enabled > 1) {
    return 'hybrid';
  }
  if (cadence.pollEnabled) {
    return 'poll';
  }
  if (cadence.allowChildPush) {
    return 'push';
  }
  return 'idle-summary';
}

export type RelationshipDescriptor = {
  id: string;
  sourceId: string;
  targetId: string;
  sourceSessionId?: string | null;
  targetSessionId?: string | null;
  sourceHandleId?: string | null;
  targetHandleId?: string | null;
  instructions: string;
  updateMode: RelationshipUpdateMode;
  pollFrequency: number;
  cadence: RelationshipCadenceConfig;
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
  hostWidthPx: number | null;
  hostHeightPx: number | null;
  cellWidthPx: number | null;
  cellHeightPx: number | null;
  quantizedCellWidthPx?: number | null;
  quantizedCellHeightPx?: number | null;
};

export type CanvasViewportState = {
  zoom: number;
  pan: { x: number; y: number };
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
  canvasViewport: CanvasViewportState;
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
