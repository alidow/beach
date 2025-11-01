'use client';

import dynamic from 'next/dynamic';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import ReactFlow, {
  Background,
  Controls,
  MiniMap,
  ReactFlowProvider,
  useReactFlow,
  type Edge as RFEdge,
  type Node as RFNode,
  type NodeProps,
  type NodeChange,
  type NodeTypes,
  type OnSelectionChangeParams,
  type Viewport,
  applyNodeChanges,
  useOnViewportChange,
} from 'reactflow';
import 'reactflow/dist/style.css';

import type { SessionSummary } from '../lib/api';
import type { SessionCredentialOverride, TerminalViewerState } from '../hooks/terminalViewerTypes';
import {
  GroupNode,
  applyAssignmentResults,
  applyDrop,
  fulfillPendingAssignment,
  groupSelection,
  previewDropTarget,
  recomputeGroupBox,
  removeOptimisticAssignment,
  summarizeAssignmentFailures,
  ungroupSelection,
  withUpdatedTimestamp,
  type CanvasLayout as SharedCanvasLayout,
  type CanvasNodeBase,
  type DropTarget,
  type PendingAssignment,
} from '../canvas';
import {
  CanvasProvider,
  useCanvasActions,
  useCanvasHandlers,
  useCanvasState,
  useRegisterCanvasHandlers,
} from '../canvas/state';
import type { CanvasLayout as ApiCanvasLayout } from '../lib/api';
import { emitTelemetry } from '../lib/telemetry';
import { extractTerminalStateDiff, type TerminalStateDiff } from '../lib/terminalHydrator';
import {
  sessionTileController,
  useCanvasSnapshot,
  useTileSnapshot,
  type TileMeasurementPayload,
} from '../controllers/sessionTileController';

const SessionTerminalPreview = dynamic(
  () => import('./SessionTerminalPreview').then((mod) => mod.SessionTerminalPreview),
  { ssr: false },
);

const TILE_PREFIX = 'tile:';
const AGENT_PREFIX = 'agent:';
const GROUP_PREFIX = 'group:';
const DEFAULT_TILE_WIDTH = 448;
const DEFAULT_TILE_HEIGHT = 448;
const DEFAULT_AGENT_WIDTH = 240;
const DEFAULT_AGENT_HEIGHT = 140;
const VIEWPORT_PAN_EPSILON = 0.5;
const VIEWPORT_ZOOM_EPSILON = 0.001;

type HandlersConfig = Parameters<typeof useRegisterCanvasHandlers>[0];

type CanvasSurfaceProps = {
  tiles: SessionSummary[];
  agents: SessionSummary[];
  layout: ApiCanvasLayout | null;
  onLayoutChange?: (layout: ApiCanvasLayout) => void;
  onPersistLayout?: (layout: ApiCanvasLayout) => void;
  onRemove: (sessionId: string) => void;
  onSelect: (session: SessionSummary) => void;
  privateBeachId: string | null;
  managerToken: string | null;
  managerUrl: string;
  viewerToken: string | null;
  viewerOverrides?: Record<string, SessionCredentialOverride | null | undefined>;
  viewerStateOverrides?: Record<string, TerminalViewerState | null | undefined>;
  handlers?: HandlersConfig;
};

type TileNodeData = {
  tileId: string;
  session: SessionSummary | null;
  onRemove: (sessionId: string) => void;
  onSelect: (session: SessionSummary) => void;
  isDropTarget: boolean;
  isDragging?: boolean;
  managerUrl: string;
  viewerToken: string | null;
  credentialOverride?: SessionCredentialOverride | null;
  viewerOverride?: TerminalViewerState | null;
  privateBeachId: string | null;
  cachedDiff?: TerminalStateDiff | null;
};

type AgentNodeData = {
  session: SessionSummary | null;
  isDropTarget: boolean;
};

type GroupNodeData = {
  name?: string;
  width: number;
  height: number;
  padding?: number;
  members: { id: string; x: number; y: number; w: number; h: number }[];
  isDropTarget: boolean;
};

function encodeNodeId(kind: 'tile' | 'agent' | 'group', id: string): string {
  if (kind === 'tile') return `${TILE_PREFIX}${id}`;
  if (kind === 'agent') return `${AGENT_PREFIX}${id}`;
  return `${GROUP_PREFIX}${id}`;
}

function decodeNodeId(nodeId: string): { kind: 'tile' | 'agent' | 'group'; id: string } | null {
  if (nodeId.startsWith(TILE_PREFIX)) {
    return { kind: 'tile', id: nodeId.slice(TILE_PREFIX.length) };
  }
  if (nodeId.startsWith(AGENT_PREFIX)) {
    return { kind: 'agent', id: nodeId.slice(AGENT_PREFIX.length) };
  }
  if (nodeId.startsWith(GROUP_PREFIX)) {
    return { kind: 'group', id: nodeId.slice(GROUP_PREFIX.length) };
  }
  return null;
}

function layoutViewport(layout: SharedCanvasLayout) {
  const pan = layout.viewport?.pan ?? { x: 0, y: 0 };
  return {
    x: pan.x ?? 0,
    y: pan.y ?? 0,
    zoom: layout.viewport?.zoom ?? 1,
  };
}

function buildCanvasNodes(layout: SharedCanvasLayout, sessionMap: Map<string, SessionSummary>, agentMap: Map<string, SessionSummary>): CanvasNodeBase[] {
  const nodes: CanvasNodeBase[] = [];
  for (const tile of Object.values(layout.tiles)) {
    nodes.push({
      id: encodeNodeId('tile', tile.id),
      type: 'tile',
      xPx: tile.position?.x ?? 0,
      yPx: tile.position?.y ?? 0,
      widthPx: tile.size?.width ?? DEFAULT_TILE_WIDTH,
      heightPx: tile.size?.height ?? DEFAULT_TILE_HEIGHT,
      zIndex: tile.zIndex,
      data: {
        sessionId: tile.id,
        title: sessionMap.get(tile.id)?.metadata?.title ?? null,
      },
    });
  }
  for (const agent of Object.values(layout.agents)) {
    nodes.push({
      id: encodeNodeId('agent', agent.id),
      type: 'agent',
      xPx: agent.position?.x ?? 0,
      yPx: agent.position?.y ?? 0,
      widthPx: agent.size?.width ?? DEFAULT_AGENT_WIDTH,
      heightPx: agent.size?.height ?? DEFAULT_AGENT_HEIGHT,
      zIndex: agent.zIndex,
      data: {
        sessionId: agent.id,
        label: agentMap.get(agent.id)?.metadata?.title ?? null,
      },
    });
  }
  for (const group of Object.values(layout.groups)) {
    nodes.push({
      id: encodeNodeId('group', group.id),
      type: 'group',
      xPx: group.position.x,
      yPx: group.position.y,
      widthPx: group.size.width,
      heightPx: group.size.height,
      zIndex: group.zIndex,
      data: {
        groupId: group.id,
        memberIds: group.memberIds,
        padding: group.padding ?? 16,
        name: group.name ?? 'Group',
      },
    });
  }
  return nodes;
}

export function getTileBorderClass({
  selected,
  isDropTarget,
  isDragging = false,
}: {
  selected: boolean;
  isDropTarget: boolean;
  isDragging?: boolean;
}) {
  if (isDragging) {
    return 'border-sky-400/70 shadow-[0_0_0_1px_rgba(56,189,248,0.3)]';
  }
  if (isDropTarget) {
    return 'border-sky-500/60 shadow-[0_0_0_1px_rgba(14,165,233,0.25)]';
  }
  if (selected) {
    return 'border-sky-400/60';
  }
  return 'border-border/80';
}

function TileNodeComponent({ data, selected }: NodeProps<TileNodeData>) {
  const {
    tileId,
    session,
    onRemove,
    onSelect,
    isDropTarget,
    isDragging,
    managerUrl,
    viewerToken,
    credentialOverride,
    viewerOverride,
    privateBeachId,
    cachedDiff,
  } = data;
  const snapshot = useTileSnapshot(tileId);
  const sessionSummary = session ?? snapshot.session;
  const borderClass = getTileBorderClass({ selected, isDropTarget, isDragging: !!isDragging });

  const effectiveViewer = viewerOverride ?? snapshot.viewer;
  const effectiveCachedDiff = cachedDiff ?? snapshot.cachedDiff ?? null;

  const handleMeasurements = useCallback(
    (_sessionId: string, measurements: unknown) => {
      if (!measurements) {
        return;
      }
      sessionTileController.enqueueMeasurement(tileId, measurements as TileMeasurementPayload, 'dom');
    },
    [tileId],
  );

  if (!sessionSummary) {
    return (
      <div className={`flex h-full w-full items-center justify-center rounded-xl border ${borderClass} bg-muted/20 text-xs text-muted-foreground`}>
        Missing session
      </div>
    );
  }

  return (
    <div className={`flex h-full w-full flex-col overflow-hidden rounded-xl border ${borderClass} bg-card shadow-sm`}>
      <div className="flex items-center justify-between gap-2 border-b border-border/60 px-3 py-2">
        <button
          type="button"
          className="truncate text-left text-sm font-medium"
          onClick={() => onSelect(sessionSummary)}
        >
          {sessionSummary.metadata?.title || sessionSummary.session_id}
        </button>
        <button
          type="button"
          className="text-xs text-muted-foreground transition hover:text-destructive"
          onClick={() => onRemove(sessionSummary.session_id)}
        >
          Remove
        </button>
      </div>
      <div className="flex flex-1 min-h-0 flex-col">
        <SessionTerminalPreview
          sessionId={sessionSummary.session_id}
          privateBeachId={privateBeachId ?? sessionSummary.private_beach_id}
          managerUrl={managerUrl}
          token={viewerToken}
          credentialOverride={credentialOverride ?? undefined}
          viewer={effectiveViewer}
          variant="preview"
          cachedStateDiff={effectiveCachedDiff ?? undefined}
          onPreviewMeasurementsChange={handleMeasurements}
        />
      </div>
    </div>
  );
}

function AgentNodeComponent({ data, selected }: NodeProps<AgentNodeData>) {
  const { session, isDropTarget } = data;
  const borderClass = isDropTarget
    ? 'border-sky-500 ring-2 ring-sky-400/30'
    : selected
      ? 'border-primary/70 ring-2 ring-primary/30'
      : 'border-border';

  return (
    <div className={`flex h-full w-full flex-col rounded-xl border ${borderClass} bg-card shadow-sm px-3 py-2`}> 
      <div className="text-xs uppercase text-muted-foreground">Agent</div>
      <div className="truncate text-sm font-medium">
        {session?.metadata?.title || session?.session_id || 'Unknown agent'}
      </div>
    </div>
  );
}

function GroupNodeComponent({ data, selected }: NodeProps<GroupNodeData>) {
  const { name, width, height, padding, members, isDropTarget } = data;
  return (
    <GroupNode
      name={name}
      width={width}
      height={height}
      padding={padding}
      members={members}
      selected={selected || isDropTarget}
    />
  );
}

const nodeTypes: NodeTypes = {
  tile: TileNodeComponent,
  agent: AgentNodeComponent,
  group: GroupNodeComponent,
};

function CanvasSurfaceInner(props: Omit<CanvasSurfaceProps, 'handlers'>) {
  const {
    tiles,
    agents,
    layout: layoutProp,
    onLayoutChange,
    onPersistLayout,
    onRemove,
    onSelect,
    privateBeachId,
    managerToken,
    managerUrl,
    viewerToken,
    viewerOverrides,
    viewerStateOverrides: viewerStateOverridesProp,
  } = props;
  const reactFlow = useReactFlow();
  const { load, setNodes, setViewport, setSelection } = useCanvasActions();
  const { selection } = useCanvasState();
  const { onDropNode, onCreateGroup, onAssignAgent, onAssignmentError } = useCanvasHandlers();
  const { layout } = useCanvasSnapshot();
  const [miniMapVisible, setMiniMapVisible] = useState(true);
  const [hoverTarget, setHoverTarget] = useState<DropTarget | null>(null);
  const [activeDragNodeId, setActiveDragNodeId] = useState<string | null>(null);
  const didLoadRef = useRef(false);
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const dragStateRef = useRef<{
    nodeId: string;
    kind: 'tile' | 'agent' | 'group';
    origin: { x: number; y: number };
    entityId: string;
  } | null>(null);
  const lastSyncNodeIdsRef = useRef<Set<string>>(new Set());
  const lastLayoutSignatureRef = useRef<string | null>(null);
  const lastSyncedViewportRef = useRef<{ x: number; y: number; zoom: number } | null>(null);
  const hydrateKeyRef = useRef<string | null>(null);
  const persistCallbackRef = useRef<typeof onPersistLayout | undefined>(onPersistLayout);
  const layoutChangeCallbackRef = useRef<typeof onLayoutChange | undefined>(onLayoutChange);

  const viewerOverrideSignature = useMemo(() => {
    if (!viewerOverrides) {
      return '';
    }
    return Object.entries(viewerOverrides)
      .map(([id, override]) => {
        if (!override) {
          return `${id}:null`;
        }
        return [
          id,
          override.authorizationToken ?? '',
          override.passcode ?? '',
          override.viewerToken ?? '',
          override.skipCredentialFetch ? '1' : '0',
        ].join(':');
      })
      .sort()
      .join('|');
  }, [viewerOverrides]);

  const viewerStateSignature = useMemo(() => {
    if (!viewerStateOverridesProp) {
      return '';
    }
    return Object.entries(viewerStateOverridesProp)
      .map(([id, state]) => `${id}:${state?.status ?? 'null'}:${state?.connecting ? '1' : '0'}`)
      .sort()
      .join('|');
  }, [viewerStateOverridesProp]);

  const hydrateKey = useMemo(() => {
    const layoutUpdatedAt = layoutProp?.metadata?.updatedAt ?? 0;
    const layoutVersion = layoutProp?.version ?? 0;
    const tileIdsSignature = [...tiles].map((session) => session.session_id).sort().join(',');
    const agentIdsSignature = [...agents].map((session) => session.session_id).sort().join(',');
    return [
      layoutVersion,
      layoutUpdatedAt,
      tileIdsSignature,
      agentIdsSignature,
      managerUrl,
      managerToken ?? '',
      viewerToken ?? '',
      privateBeachId ?? '',
      viewerOverrideSignature,
      viewerStateSignature,
    ].join('|');
  }, [
    agents,
    layoutProp?.metadata?.updatedAt,
    layoutProp?.version,
    managerToken,
    managerUrl,
    privateBeachId,
    tiles,
    viewerOverrideSignature,
    viewerStateSignature,
    viewerToken,
  ]);

  const sessionMap = useMemo(() => new Map(tiles.map((session) => [session.session_id, session] as const)), [tiles]);
  const agentMap = useMemo(() => new Map(agents.map((session) => [session.session_id, session] as const)), [agents]);

  const cachedTerminalDiffs = useMemo<Record<string, TerminalStateDiff>>(() => {
    const map: Record<string, TerminalStateDiff> = {};
    const seen = new Set<string>();
    for (const session of tiles) {
      const diff =
        extractTerminalStateDiff(session.last_state) ?? extractTerminalStateDiff(session.metadata);
      if (diff) {
        map[session.session_id] = diff;
        seen.add(session.session_id);
        if (typeof window !== 'undefined') {
          console.info('[terminal-hydrate][canvas][session]', {
            sessionId: session.session_id,
            sequence: diff.sequence ?? null,
          });
        }
      } else if (typeof window !== 'undefined') {
        console.info('[terminal-hydrate][canvas][session-miss]', {
          sessionId: session.session_id,
        });
      }
    }
    const layoutTiles = layout?.tiles ?? {};
    for (const [tileId, tile] of Object.entries(layoutTiles)) {
      if (seen.has(tileId)) {
        continue;
      }
      const diff = extractTerminalStateDiff(tile?.metadata ?? null);
      if (diff) {
        map[tileId] = diff;
        if (typeof window !== 'undefined') {
          console.info('[terminal-hydrate][canvas][layout]', {
            sessionId: tileId,
            sequence: diff.sequence ?? null,
          });
        }
      }
    }
    return map;
  }, [layout?.tiles, tiles]);

  useEffect(() => {
    for (const [sessionId, diff] of Object.entries(cachedTerminalDiffs)) {
      sessionTileController.setCachedDiff(sessionId, diff ?? null);
    }
  }, [cachedTerminalDiffs]);

  useEffect(() => {
    const keyChanged = hydrateKeyRef.current !== hydrateKey;
    const callbacksChanged =
      persistCallbackRef.current !== onPersistLayout ||
      layoutChangeCallbackRef.current !== onLayoutChange;
    if (!keyChanged && !callbacksChanged) {
      return;
    }
    hydrateKeyRef.current = hydrateKey;
    persistCallbackRef.current = onPersistLayout;
    layoutChangeCallbackRef.current = onLayoutChange;
    sessionTileController.hydrate({
      layout: layoutProp,
      sessions: tiles,
      agents,
      privateBeachId,
      managerUrl,
      managerToken,
      viewerToken,
      viewerOverrides,
      viewerStateOverrides: viewerStateOverridesProp,
      cachedDiffs: cachedTerminalDiffs,
      onPersistLayout,
      onLayoutChange,
    });
  }, [
    cachedTerminalDiffs,
    agents,
    hydrateKey,
    layoutProp,
    managerToken,
    managerUrl,
    onLayoutChange,
    onPersistLayout,
    privateBeachId,
    tiles,
    viewerOverrides,
    viewerStateOverridesProp,
    viewerToken,
  ]);

  const viewerStateOverridesResolved = useMemo(() => {
    if (!viewerStateOverridesProp) {
      return {} as Record<string, TerminalViewerState | null | undefined>;
    }
    return { ...viewerStateOverridesProp } as Record<string, TerminalViewerState | null | undefined>;
  }, [viewerStateOverridesProp]);

  const effectiveViewerStateOverrides = viewerStateOverridesResolved;

  const syncStore = useCallback(
    (next: SharedCanvasLayout) => {
      const nodes = buildCanvasNodes(next, sessionMap, agentMap);
      const viewport = layoutViewport(next);
      if (typeof window !== 'undefined') {
        const prevIds = lastSyncNodeIdsRef.current;
        const nextIds = new Set(nodes.map((node) => node.id));
        const added: string[] = [];
        const removed: string[] = [];
        nextIds.forEach((id) => {
          if (!prevIds.has(id)) {
            added.push(id);
          }
        });
        prevIds.forEach((id) => {
          if (!nextIds.has(id)) {
            removed.push(id);
          }
        });
        lastSyncNodeIdsRef.current = nextIds;
        const layoutSignature = JSON.stringify({
          tiles: Object.keys(next.tiles),
          agents: Object.keys(next.agents),
          groups: Object.keys(next.groups),
        });
        const previousSignature = lastLayoutSignatureRef.current;
        lastLayoutSignatureRef.current = layoutSignature;
        console.info('[canvas-sync] apply', {
          added,
          removed,
          nodeCount: nodes.length,
          viewport,
          previousSignature,
          nextSignature: layoutSignature,
        });
      }
      if (!didLoadRef.current) {
        load({ version: 3, nodes, edges: next.edges ?? [], viewport });
        didLoadRef.current = true;
        lastSyncedViewportRef.current = viewport;
      } else {
        setNodes(nodes);
        const prevViewport = lastSyncedViewportRef.current;
        const shouldSyncViewport =
          !prevViewport ||
          Math.abs(prevViewport.x - viewport.x) > 0.5 ||
          Math.abs(prevViewport.y - viewport.y) > 0.5 ||
          Math.abs(prevViewport.zoom - viewport.zoom) > 0.001;
        if (shouldSyncViewport) {
          setViewport(viewport);
        }
        lastSyncedViewportRef.current = viewport;
      }
    },
    [agentMap, load, sessionMap, setNodes, setViewport],
  );

  useEffect(() => {
    syncStore(layout);
  }, [layout, syncStore]);

  const updateLayout = useCallback(
    (reason: string, produce: (current: SharedCanvasLayout) => SharedCanvasLayout) => {
      sessionTileController.updateLayout(reason, produce);
    },
    [],
  );

  const toCanvasPoint = useCallback(
    (clientX: number, clientY: number) => {
      const bounds = wrapperRef.current?.getBoundingClientRect();
      const x = clientX - (bounds?.left ?? 0);
      const y = clientY - (bounds?.top ?? 0);
      return reactFlow.project({ x, y });
    },
    [reactFlow],
  );

  const adapter = useMemo(() => ({ toCanvasPoint }), [toCanvasPoint]);

  const selectionSet = useMemo(() => new Set(selection), [selection]);

  const rfNodes = useMemo(() => {
    const nodes: RFNode[] = [];
    for (const tile of Object.values(layout.tiles)) {
      const nodeId = encodeNodeId('tile', tile.id);
      const session = sessionMap.get(tile.id) ?? null;
      const isActiveDrag = activeDragNodeId === nodeId;
      nodes.push({
        id: nodeId,
        type: 'tile',
        position: { x: tile.position?.x ?? 0, y: tile.position?.y ?? 0 },
        data: {
          tileId: tile.id,
          session,
          onRemove,
          onSelect,
          isDropTarget: hoverTarget?.type === 'tile' && hoverTarget.id === tile.id,
          isDragging: isActiveDrag,
          managerUrl,
          viewerToken,
          credentialOverride: viewerOverrides?.[tile.id] ?? null,
          viewerOverride: effectiveViewerStateOverrides[tile.id] ?? null,
          privateBeachId,
          cachedDiff: cachedTerminalDiffs[tile.id] ?? null,
        } satisfies TileNodeData,
        draggable: true,
        selectable: true,
        style: {
          width: tile.size?.width ?? DEFAULT_TILE_WIDTH,
          height: tile.size?.height ?? DEFAULT_TILE_HEIGHT,
          zIndex: tile.zIndex ?? 1,
        },
        selected: selectionSet.has(nodeId),
      });
    }

    for (const agent of Object.values(layout.agents)) {
      const nodeId = encodeNodeId('agent', agent.id);
      const session = agentMap.get(agent.id) ?? null;
      nodes.push({
        id: nodeId,
        type: 'agent',
        position: { x: agent.position?.x ?? 0, y: agent.position?.y ?? 0 },
        data: {
          session,
          isDropTarget: hoverTarget?.type === 'agent' && hoverTarget.id === agent.id,
        } satisfies AgentNodeData,
        draggable: true,
        selectable: true,
        style: {
          width: agent.size?.width ?? DEFAULT_AGENT_WIDTH,
          height: agent.size?.height ?? DEFAULT_AGENT_HEIGHT,
          zIndex: agent.zIndex ?? 1,
        },
        selected: selectionSet.has(nodeId),
      });
    }

    for (const group of Object.values(layout.groups)) {
      const nodeId = encodeNodeId('group', group.id);
      const members = group.memberIds
        .map((memberId) => layout.tiles[memberId])
        .filter(Boolean)
        .map((tile) => ({
          id: tile!.id,
          x: tile!.position.x - group.position.x,
          y: tile!.position.y - group.position.y,
          w: tile!.size.width,
          h: tile!.size.height,
        }));
      nodes.push({
        id: nodeId,
        type: 'group',
        position: { x: group.position.x, y: group.position.y },
        data: {
          name: group.name,
          width: group.size.width,
          height: group.size.height,
          padding: group.padding ?? 16,
          members,
          isDropTarget: hoverTarget?.type === 'group' && hoverTarget.id === group.id,
        } satisfies GroupNodeData,
        draggable: true,
        selectable: true,
        style: {
          width: group.size.width,
          height: group.size.height,
          zIndex: group.zIndex ?? 1,
        },
        selected: selectionSet.has(nodeId),
      });
    }

    return nodes;
  }, [activeDragNodeId, agentMap, cachedTerminalDiffs, hoverTarget, layout.agents, layout.groups, layout.tiles, managerUrl, onRemove, onSelect, privateBeachId, selectionSet, sessionMap, viewerOverrides, effectiveViewerStateOverrides, viewerToken]);

  const edges = useMemo<RFEdge[]>(() => [], []);

  const handleNodesChange = useCallback(
    (changes: NodeChange[]) => {
      setNodes((prev) => applyNodeChanges(changes, prev));
    },
    [setNodes],
  );

  const handleSelectionChange = useCallback(
    ({ nodes }: OnSelectionChangeParams) => {
      setSelection(nodes.map((node) => node.id));
    },
    [setSelection],
  );

  const handleNodeDragStart = useCallback(
    (_event: React.MouseEvent, node: RFNode) => {
      if (node && typeof node.id === 'string') {
        setActiveDragNodeId(node.id);
      }
    },
    [],
  );

  const handleNodeDrag = useCallback(
    (event: React.MouseEvent, node: RFNode) => {
      if (!node || typeof node.id !== 'string') {
        setHoverTarget(null);
        return;
      }
      const parsed = decodeNodeId(node.id);
      if (!parsed) {
        setHoverTarget(null);
        return;
      }

      let updatedLayout: SharedCanvasLayout | null = null;
      sessionTileController.updateLayout('drag-preview-position', (current) => {
        const tile = current.tiles[parsed.id];
        if (!tile) {
          return current;
        }
        const nextPosition = { x: node.position.x, y: node.position.y };
        if (tile.position.x === nextPosition.x && tile.position.y === nextPosition.y) {
          return current;
        }
        const next: SharedCanvasLayout = {
          ...current,
          tiles: {
            ...current.tiles,
            [parsed.id]: {
              ...tile,
              position: nextPosition,
            },
          },
        };
        updatedLayout = next;
        return next;
      });

      const layoutForPreview = updatedLayout ?? layout;

      if (!dragStateRef.current || dragStateRef.current.nodeId !== node.id) {
        let origin = { x: node.position.x, y: node.position.y };
        const payload: Record<string, unknown> = {
          beachId: privateBeachId ?? undefined,
          nodeType: parsed.kind,
          nodeId: parsed.id,
        };
        if (parsed.kind === 'tile') {
          const tile = layout.tiles[parsed.id];
          if (tile) origin = tile.position;
          if (tile) {
            payload.width = tile.size.width;
            payload.height = tile.size.height;
          }
        } else if (parsed.kind === 'agent') {
          const agent = layout.agents[parsed.id];
          if (agent) origin = agent.position;
        } else if (parsed.kind === 'group') {
          const group = layout.groups[parsed.id];
          if (group) origin = group.position;
          if (group) {
            payload.width = group.size.width;
            payload.height = group.size.height;
          }
        }
        dragStateRef.current = {
          nodeId: node.id,
          kind: parsed.kind,
          origin,
          entityId: parsed.id,
        };
        emitTelemetry('canvas.drag.start', {
          ...payload,
          x: origin.x,
          y: origin.y,
        });
      }

      const target = previewDropTarget(layoutForPreview, adapter, event.clientX, event.clientY);
      if (parsed.kind === 'tile' && target.type === 'tile' && target.id === parsed.id) {
        setHoverTarget(null);
        return;
      }
      setHoverTarget(target.type === 'none' ? null : target);
    },
    [adapter, layout, privateBeachId],
  );

  const processPendingAssignment = useCallback(
    (pending: PendingAssignment | undefined, snapshot: SharedCanvasLayout) => {
      if (!pending) {
        return;
      }
      if (!managerToken || managerToken.trim().length === 0) {
        updateLayout('assignment-rollback-no-token', (current) =>
          removeOptimisticAssignment(current, pending.controllerId, pending.target),
        );
        onAssignmentError?.('Missing manager auth token.');
        return;
      }
      void (async () => {
        try {
          const response = await fulfillPendingAssignment(snapshot, pending, managerToken, managerUrl, {
            privateBeachId: privateBeachId ?? undefined,
          });
          updateLayout('assignment-apply-results', (current) =>
            applyAssignmentResults(current, pending, response),
          );
          const successIds = response.results.filter((result) => result.ok).map((result) => result.childId);
          if (successIds.length > 0) {
            onAssignAgent?.({ agentId: pending.controllerId, targetIds: successIds, response });
          }
          const failureMessage = summarizeAssignmentFailures(response);
          onAssignmentError?.(failureMessage ?? null);
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          updateLayout('assignment-rollback-error', (current) =>
            removeOptimisticAssignment(current, pending.controllerId, pending.target),
          );
          onAssignmentError?.(`Assignment failed: ${message}`);
        }
      })();
    },
    [managerToken, managerUrl, onAssignAgent, onAssignmentError, privateBeachId, updateLayout],
  );

  const handleNodeDragStop = useCallback(
    (event: React.MouseEvent, node: RFNode) => {
      setHoverTarget(null);
      setActiveDragNodeId(null);
      if (!node || typeof node.id !== 'string') {
        dragStateRef.current = null;
        return;
      }
      const parsed = decodeNodeId(node.id);
      if (!parsed) {
        dragStateRef.current = null;
        return;
      }
      if (parsed.kind === 'tile') {
        let pending: PendingAssignment | undefined;
        let snapshot: SharedCanvasLayout | null = null;
        updateLayout('drag-stop-tile', (current) => {
          const tile = current.tiles[parsed.id];
          if (!tile) return current;
          let next: SharedCanvasLayout = {
            ...current,
            tiles: {
              ...current.tiles,
              [parsed.id]: {
                ...tile,
                position: { x: node.position.x, y: node.position.y },
              },
            },
          };
          if (tile.groupId) {
            next = recomputeGroupBox(next, tile.groupId);
          }
          next = withUpdatedTimestamp(next);
          const target = previewDropTarget(next, adapter, event.clientX, event.clientY);
          const beforeGroups = new Set(Object.keys(current.groups));
          const dropResult = applyDrop(next, { type: 'tile', id: parsed.id }, target);
          snapshot = dropResult.layout;
          pending = dropResult.pendingAssignment;
          const newGroupId = Object.keys(dropResult.layout.groups).find((id) => !beforeGroups.has(id));
          if (newGroupId) {
            onCreateGroup?.({ memberIds: dropResult.layout.groups[newGroupId].memberIds, name: dropResult.layout.groups[newGroupId].name });
          }
          if (target.type !== 'none') {
            onDropNode?.({ sourceId: parsed.id, targetId: target.id, kind: target.type });
          }
          return dropResult.layout;
        });
        if (pending && snapshot) {
          processPendingAssignment(pending, snapshot);
        }
      } else if (parsed.kind === 'agent') {
        updateLayout('drag-stop-agent', (current) => {
          const agent = current.agents[parsed.id];
          if (!agent) return current;
          return withUpdatedTimestamp({
            ...current,
            agents: {
              ...current.agents,
              [parsed.id]: {
                ...agent,
                position: { x: node.position.x, y: node.position.y },
              },
            },
          });
        });
      } else if (parsed.kind === 'group') {
        let pending: PendingAssignment | undefined;
        let snapshot: SharedCanvasLayout | null = null;
        updateLayout('drag-stop-group', (current) => {
          const group = current.groups[parsed.id];
          if (!group) return current;
          const dx = node.position.x - group.position.x;
          const dy = node.position.y - group.position.y;
          const tiles = { ...current.tiles };
          for (const memberId of group.memberIds) {
            const tile = tiles[memberId];
            if (tile) {
              tiles[memberId] = {
                ...tile,
                position: {
                  x: tile.position.x + dx,
                  y: tile.position.y + dy,
                },
              };
            }
          }
          let next: SharedCanvasLayout = {
            ...current,
            groups: {
              ...current.groups,
              [parsed.id]: {
                ...group,
                position: { x: node.position.x, y: node.position.y },
              },
            },
            tiles,
          };
          next = withUpdatedTimestamp(next);
          const target = previewDropTarget(next, adapter, event.clientX, event.clientY);
          const dropResult = applyDrop(next, { type: 'group', id: parsed.id }, target);
          snapshot = dropResult.layout;
          pending = dropResult.pendingAssignment;
          if (target.type !== 'none') {
            onDropNode?.({ sourceId: parsed.id, targetId: target.id, kind: target.type });
          }
          return dropResult.layout;
        });
        if (pending && snapshot) {
          processPendingAssignment(pending, snapshot);
        }
      }

      const dragState = dragStateRef.current;
      if (dragState && dragState.nodeId === node.id) {
        const finalPosition = { x: node.position.x, y: node.position.y };
        const payload: Record<string, unknown> = {
          beachId: privateBeachId ?? undefined,
          nodeType: dragState.kind,
          nodeId: dragState.entityId,
          fromX: dragState.origin.x,
          fromY: dragState.origin.y,
          toX: finalPosition.x,
          toY: finalPosition.y,
        };
        if (dragState.kind === 'tile') {
          const tile = layout.tiles[dragState.entityId];
          if (tile) {
            payload.width = tile.size.width;
            payload.height = tile.size.height;
          }
        } else if (dragState.kind === 'group') {
          const group = layout.groups[dragState.entityId];
          if (group) {
            payload.width = group.size.width;
            payload.height = group.size.height;
          }
        }
        emitTelemetry('canvas.drag.stop', payload);
        dragStateRef.current = null;
      }
    },
    [adapter, layout, onCreateGroup, onDropNode, processPendingAssignment, privateBeachId, updateLayout],
  );

  const handleMoveEnd = useCallback(() => {
    const viewport = reactFlow.getViewport();
    updateLayout('viewport-move', (current) =>
      withUpdatedTimestamp({
        ...current,
        viewport: {
          zoom: viewport.zoom,
          pan: { x: viewport.x, y: viewport.y },
        },
      }),
    );
  }, [reactFlow, updateLayout]);

  const handleViewportChange = useCallback((viewport: Viewport) => {
    lastSyncedViewportRef.current = viewport;
  }, []);

  const handleViewportEnd = useCallback(
    (viewport: Viewport) => {
      lastSyncedViewportRef.current = viewport;
      if (!didLoadRef.current) {
        return;
      }
      updateLayout('viewport-sync', (current) => {
        const existingPan = current.viewport?.pan ?? { x: 0, y: 0 };
        const existingZoom = current.viewport?.zoom ?? 1;
        const panDeltaX = Math.abs(existingPan.x - viewport.x);
        const panDeltaY = Math.abs(existingPan.y - viewport.y);
        const zoomDelta = Math.abs(existingZoom - viewport.zoom);
        if (
          panDeltaX < VIEWPORT_PAN_EPSILON &&
          panDeltaY < VIEWPORT_PAN_EPSILON &&
          zoomDelta < VIEWPORT_ZOOM_EPSILON
        ) {
          return current;
        }
        return withUpdatedTimestamp({
          ...current,
          viewport: {
            zoom: viewport.zoom,
            pan: { x: viewport.x, y: viewport.y },
          },
        });
      });
    },
    [updateLayout],
  );

  useOnViewportChange({
    onChange: handleViewportChange,
    onEnd: handleViewportEnd,
  });

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (!(event.metaKey || event.ctrlKey)) return;
      if (event.key.toLowerCase() !== 'g') return;
      const tileSelection = selection
        .map(decodeNodeId)
        .filter((entry): entry is { kind: 'tile'; id: string } => entry != null && entry.kind === 'tile')
        .map((entry) => entry.id);
      if (tileSelection.length === 0) return;
      event.preventDefault();
      updateLayout('keyboard-group-toggle', (current) => {
        const next = event.shiftKey ? ungroupSelection(current, tileSelection) : groupSelection(current, tileSelection);
        if (!event.shiftKey && next !== current) {
          const newGroup = Object.keys(next.groups).find((id) => !current.groups[id]);
          if (newGroup) {
            onCreateGroup?.({ memberIds: next.groups[newGroup].memberIds, name: next.groups[newGroup].name });
          }
        }
        return next;
      });
    }
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [onCreateGroup, selection, updateLayout]);

  return (
    <div ref={wrapperRef} className="relative h-full w-full">
      <ReactFlow
        nodeTypes={nodeTypes}
        nodes={rfNodes}
        edges={edges}
        onNodesChange={handleNodesChange}
        onSelectionChange={handleSelectionChange}
        onNodeDragStart={handleNodeDragStart}
        onNodeDrag={handleNodeDrag}
        onNodeDragStop={handleNodeDragStop}
        onMoveEnd={handleMoveEnd}
        panOnDrag={false}
        minZoom={0.1}
        maxZoom={2}
        proOptions={{ hideAttribution: true }}
        style={{ width: '100%', height: '100%' }}
      >
        <Background />
        <Controls />
        {miniMapVisible ? (
          <MiniMap
            pannable
            zoomable
            className="rounded-md border border-slate-800 bg-slate-950/95 text-xs shadow shadow-black/40"
            maskColor="rgba(15, 23, 42, 0.7)"
            nodeColor={(node) => {
              if (node.type === 'agent') return '#0f766e';
              if (node.type === 'group') return '#5b21b6';
              return '#1d4ed8';
            }}
            nodeStrokeColor={(node) => (node.selected ? '#f97316' : '#020617')}
          />
        ) : null}
      </ReactFlow>
      <button
        type="button"
        onClick={() => setMiniMapVisible((prev) => !prev)}
        className="absolute bottom-3 right-3 z-10 flex h-8 w-8 items-center justify-center rounded-full border border-slate-800 bg-slate-950/90 text-slate-300 shadow shadow-black/40 transition hover:bg-slate-900/70 hover:text-white"
        aria-label={miniMapVisible ? 'Hide mini map' : 'Show mini map'}
      >
        {miniMapVisible ? (
          <svg aria-hidden="true" className="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <path d="M2.458 12C3.732 7.943 7.522 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.478 0-8.268-2.943-9.542-7Z" />
            <path d="M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z" />
            <line x1="3" y1="3" x2="21" y2="21" />
          </svg>
        ) : (
          <svg aria-hidden="true" className="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <path d="M2.458 12C3.732 7.943 7.522 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.478 0-8.268-2.943-9.542-7Z" />
            <path d="M12 15c1.657 0 3-1.343 3-3s-1.343-3-3-3-3 1.343-3 3 1.343 3 3 3Z" />
          </svg>
        )}
      </button>
    </div>
  );
}

export default function CanvasSurface(props: CanvasSurfaceProps) {
  const { handlers, ...rest } = props;
  const HandlersProvider = useRegisterCanvasHandlers(handlers ?? {});
  return (
    <div className="flex h-full min-h-[520px] w-full">
      <CanvasProvider>
        <HandlersProvider>
          <ReactFlowProvider>
            <div className="flex-1 min-h-0">
              <CanvasSurfaceInner {...rest} />
            </div>
          </ReactFlowProvider>
        </HandlersProvider>
      </CanvasProvider>
    </div>
  );
}
