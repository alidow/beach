import { useRouter } from 'next/router';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useAuth } from '@clerk/nextjs';
import TopNav from '../../../components/TopNav';
import dynamic from 'next/dynamic';
const CanvasSurface = dynamic(() => import('../../../components/CanvasSurface'), {
  ssr: false,
  loading: () => <div className="h-[520px] rounded-xl border border-border bg-card shadow-sm" />,
});
import SessionDrawer from '../../../components/SessionDrawer';
import { Button } from '../../../components/ui/button';
import AddSessionModal from '../../../components/AddSessionModal';
import {
  SessionSummary,
  listSessions,
  getBeachMeta,
  getCanvasLayout,
  putCanvasLayout,
  updateBeach,
  listControllerPairingsForControllers,
  createControllerPairing,
  deleteControllerPairing,
  updateSessionRole,
  buildMetadataWithRole,
  type SessionRole,
  type ControllerPairing,
} from '../../../lib/api';
import type { CanvasLayout, ControllerUpdateCadence } from '../../../lib/api';
import type { BatchAssignmentResponse } from '../../../canvas';
import { BeachSettingsProvider, ManagerSettings } from '../../../components/settings/BeachSettingsContext';
import { BeachSettingsButton } from '../../../components/settings/SettingsButton';
import { AgentExplorer } from '../../../components/AgentExplorer';
import { AssignmentDetailPane } from '../../../components/AssignmentDetailPane';
import { debugLog, debugStack } from '../../../lib/debug';
import { useControllerPairingStreams } from '../../../hooks/useControllerPairingStreams';
import { buildAssignmentModel } from '../../../lib/assignments';

const DEFAULT_CANVAS_TILE_WIDTH = 448;
const DEFAULT_CANVAS_TILE_HEIGHT = 448;
const DEFAULT_CANVAS_TILE_GAP = 32;
const AUTO_LAYOUT_COLUMNS = 4;

function emptyCanvasLayout(): CanvasLayout {
  const now = Date.now();
  return {
    version: 3,
    viewport: { zoom: 1, pan: { x: 0, y: 0 } },
    tiles: {},
    agents: {},
    groups: {},
    controlAssignments: {},
    metadata: { createdAt: now, updatedAt: now },
  };
}

function ensureLayoutMetadata(layout: CanvasLayout | null): CanvasLayout {
  if (!layout) {
    return emptyCanvasLayout();
  }
  const createdAt = layout.metadata?.createdAt ?? Date.now();
  const updatedAt = layout.metadata?.updatedAt ?? createdAt;
  const normalizedTiles: CanvasLayout['tiles'] = {};
  for (const [tileId, tile] of Object.entries(layout.tiles ?? {})) {
    const normalizedMetadata =
      tile && typeof tile === 'object' && 'metadata' in tile && tile.metadata && typeof tile.metadata === 'object'
        ? { ...(tile.metadata as Record<string, any>) }
        : undefined;
    normalizedTiles[tileId] = {
      kind: tile.kind ?? 'application',
      id: tile.id ?? tileId,
      position: tile.position ?? { x: 0, y: 0 },
      size: tile.size ?? { width: DEFAULT_CANVAS_TILE_WIDTH, height: DEFAULT_CANVAS_TILE_HEIGHT },
      zIndex: tile.zIndex ?? 1,
      groupId: tile.groupId,
      zoom: tile.zoom,
      locked: tile.locked,
      toolbarPinned: tile.toolbarPinned,
      metadata: normalizedMetadata,
    };
  }
  return {
    version: 3,
    viewport: layout.viewport ?? { zoom: 1, pan: { x: 0, y: 0 } },
    tiles: normalizedTiles,
    agents: layout.agents ?? {},
    groups: layout.groups ?? {},
    controlAssignments: layout.controlAssignments ?? {},
    metadata: {
      createdAt,
      updatedAt,
      migratedFrom: layout.metadata?.migratedFrom,
    },
  };
}

function withUpdatedTimestamp(layout: CanvasLayout): CanvasLayout {
  const base = ensureLayoutMetadata(layout);
  return {
    ...base,
    metadata: {
      ...base.metadata,
      updatedAt: Date.now(),
    },
  };
}

function computeAutoPosition(index: number) {
  const column = index % AUTO_LAYOUT_COLUMNS;
  const row = Math.floor(index / AUTO_LAYOUT_COLUMNS);
  return {
    x: column * (DEFAULT_CANVAS_TILE_WIDTH + DEFAULT_CANVAS_TILE_GAP),
    y: row * (DEFAULT_CANVAS_TILE_HEIGHT + DEFAULT_CANVAS_TILE_GAP),
  };
}

export default function BeachDashboard() {
  const router = useRouter();
  const { id } = router.query as { id?: string };
  const [beachName, setBeachName] = useState<string>('');
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [canvasLayout, setCanvasLayout] = useState<CanvasLayout | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [selected, setSelected] = useState<SessionSummary | null>(null);
  const [addOpen, setAddOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const defaultManagerUrl = process.env.NEXT_PUBLIC_MANAGER_URL || 'http://localhost:8080';
  const defaultRoadUrl =
    process.env.NEXT_PUBLIC_ROAD_URL ||
    process.env.NEXT_PUBLIC_SESSION_SERVER_URL ||
    'https://api.beach.sh';
  const [settingsJson, setSettingsJson] = useState<any>({});
  const [savingSettings, setSavingSettings] = useState(false);
  const { isLoaded, isSignedIn, getToken } = useAuth();
  const tokenTemplate = process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE;
  const [managerToken, setManagerToken] = useState<string | null>(null);
  const [viewerToken, setViewerToken] = useState<string | null>(null);
  const [assignments, setAssignments] = useState<ControllerPairing[]>([]);
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const [selectedApplicationId, setSelectedApplicationId] = useState<string | null>(null);
  const [activeAssignment, setActiveAssignment] = useState<ControllerPairing | null>(null);
  const [assignmentPaneOpen, setAssignmentPaneOpen] = useState(false);
  const [assignmentSaving, setAssignmentSaving] = useState(false);
  const [assignmentError, setAssignmentError] = useState<string | null>(null);
  const canvasLayoutRef = useRef<CanvasLayout | null>(null);
  const formatAssignmentError = useCallback((message: string) => {
    if (message === 'controller_pairing_api_unavailable') {
      return 'Controller pairing API is not enabled on this manager build yet. Coordinate with Track A or update your backend stubs.';
    }
    return message;
  }, []);

  const managerSettings = useMemo<ManagerSettings>(() => {
    const raw = settingsJson && typeof settingsJson === 'object' ? (settingsJson.manager as any) || {} : {};
    return {
      managerUrl:
        typeof raw?.manager_url === 'string' && raw.manager_url.trim().length > 0
          ? raw.manager_url.trim()
          : defaultManagerUrl,
      roadUrl:
        typeof raw?.road_url === 'string' && raw.road_url.trim().length > 0
          ? raw.road_url.trim()
          : defaultRoadUrl,
    };
  }, [settingsJson, defaultManagerUrl, defaultRoadUrl]);

  const managerUrl = managerSettings.managerUrl;
  const roadUrl = managerSettings.roadUrl;

  useEffect(() => {
    canvasLayoutRef.current = canvasLayout;
  }, [canvasLayout]);

  const refreshManagerToken = useCallback(async () => {
    if (!isLoaded || !isSignedIn) {
      debugLog(
        'auth',
        'manager token refresh skipped (auth not ready)',
        {
          isLoaded,
          isSignedIn,
        },
      );
      setManagerToken(null);
      setViewerToken(null);
      if (typeof window !== 'undefined') {
        console.info('[auth] manager-token-state', {
          source: 'refresh-skip',
          reason: 'auth-not-ready',
        });
      }
      return null;
    }
    try {
      const token = await getToken(
        tokenTemplate ? { template: tokenTemplate } : undefined,
      );
      debugLog(
        'auth',
        'manager token refresh resolved',
        {
          hasToken: Boolean(token),
        },
      );
      setManagerToken(token ?? null);
      if (typeof window !== 'undefined') {
        console.info('[auth] manager-token-state', {
          source: 'refresh-success',
          hasToken: Boolean(token && token.trim().length > 0),
        });
      }
      if (token && token.trim().length > 0) {
        setViewerToken((prev) => {
          const reuseExisting = Boolean(prev && prev.trim().length > 0);
          if (typeof window !== 'undefined') {
            console.info('[auth] viewer-token-state', {
              source: 'refresh-success',
              reusedPrevious: reuseExisting,
            });
          }
          if (reuseExisting) {
            return prev;
          }
          return token;
        });
      } else {
        setViewerToken(null);
        if (typeof window !== 'undefined') {
          console.info('[auth] viewer-token-state', {
            source: 'refresh-success',
            cleared: true,
          });
        }
      }
      return token ?? null;
    } catch (err: any) {
      const message = err?.message ?? String(err);
      debugLog(
        'auth',
        'manager token refresh failed',
        {
          error: message,
        },
        'warn',
      );
      setManagerToken(null);
      setViewerToken(null);
      if (typeof window !== 'undefined') {
        console.info('[auth] manager-token-state', {
          source: 'refresh-failure',
          error: message,
        });
      }
      return null;
    }
  }, [isLoaded, isSignedIn, getToken, tokenTemplate]);

  const ensureManagerToken = useCallback(async () => {
    if (managerToken && managerToken.trim().length > 0) {
      return managerToken;
    }
    return await refreshManagerToken();
  }, [managerToken, refreshManagerToken]);

  useEffect(() => {
    if (!isLoaded) return;
    if (!isSignedIn) {
      router.replace('/sign-in');
    }
  }, [isLoaded, isSignedIn, router]);

  useEffect(() => {
    if (!isLoaded) return;
    refreshManagerToken().catch(() => {});
  }, [isLoaded, refreshManagerToken]);

  useEffect(() => {
    if (!isLoaded || !isSignedIn) {
      setManagerToken(null);
      setViewerToken(null);
      return;
    }
    const interval = setInterval(() => {
      refreshManagerToken().catch(() => {});
    }, 60_000);
    return () => clearInterval(interval);
  }, [isLoaded, isSignedIn, refreshManagerToken]);

  useEffect(() => {
    if (!id) return;
    if (!isLoaded || !isSignedIn) {
      debugLog(
        'dashboard',
        'meta fetch waiting for auth',
        {
          privateBeachId: id,
          isLoaded,
          isSignedIn,
        },
      );
      return;
    }
    let active = true;
    (async () => {
      try {
        const token = await ensureManagerToken();
        if (!active) {
          return;
        }
        if (!token || token.trim().length === 0) {
          debugLog(
            'dashboard',
            'meta fetch missing manager token',
            {
              privateBeachId: id,
              isLoaded,
              isSignedIn,
            },
            'warn',
          );
          setError('Not authorized to load beach.');
          setCanvasLayout(emptyCanvasLayout());
          return;
        }
        debugLog(
          'dashboard',
          'meta fetch token acquired',
          {
            privateBeachId: id,
          },
        );
        const meta = await getBeachMeta(id, token, managerSettings.managerUrl);
        if (!active) return;
        setBeachName(meta.name);
        setSettingsJson(meta.settings || {});
        try {
          const layoutResponse = await getCanvasLayout(id, token, managerSettings.managerUrl);
          if (!active) return;
          setCanvasLayout(ensureLayoutMetadata(layoutResponse));
          setError(null);
        } catch (layoutErr: any) {
          debugLog(
            'dashboard',
            'canvas layout fetch failed',
            {
              privateBeachId: id,
              error: layoutErr?.message ?? String(layoutErr),
            },
            'warn',
          );
          setCanvasLayout(emptyCanvasLayout());
        }
      } catch (e: any) {
        if (!active) return;
        const message = e?.message ?? String(e);
        debugLog(
          'dashboard',
          'meta fetch failed',
          {
            privateBeachId: id,
            error: message,
          },
          'warn',
        );
        if (message === 'not_found') setError('Beach not found');
        else if (message.includes('401')) setError('Not authorized to load beach.');
        else setError('Failed to load beach');
        setCanvasLayout(emptyCanvasLayout());
      }
    })();
    return () => {
      active = false;
    };
  }, [id, isLoaded, isSignedIn, ensureManagerToken, managerSettings.managerUrl]);

  type RefreshOverride = { managerUrl?: string };
  type RefreshMetadata = Record<string, unknown>;

  const hasLoadedSessionsRef = useRef(false);
  const knownSessionIds = useRef<Set<string>>(new Set());

  useEffect(() => {
    hasLoadedSessionsRef.current = false;
    knownSessionIds.current = new Set();
    setAssignments([]);
  }, [id]);

  const handleCanvasLayoutChange = useCallback((next: CanvasLayout) => {
    const normalized = ensureLayoutMetadata(next);
    canvasLayoutRef.current = normalized;
    setCanvasLayout(normalized);
  }, []);

  const handleCanvasAssignmentError = useCallback(
    (message: string | null) => {
      setAssignmentError(message ? formatAssignmentError(message) : null);
    },
    [formatAssignmentError],
  );

  const persistCanvasLayout = useCallback(
    async (next: CanvasLayout) => {
      if (!id) return;
      const normalized = ensureLayoutMetadata(next);
      const previous = canvasLayoutRef.current;
      canvasLayoutRef.current = normalized;
      setCanvasLayout(normalized);
      if (!managerToken) {
        return;
      }
      try {
        const saved = await putCanvasLayout(id, normalized, managerToken, managerSettings.managerUrl);
        const ensured = ensureLayoutMetadata(saved);
        canvasLayoutRef.current = ensured;
        setCanvasLayout(ensured);
      } catch (err) {
        canvasLayoutRef.current = previous ?? normalized;
        if (previous) {
          setCanvasLayout(previous);
        }
        debugLog(
          'layout',
          'putCanvasLayout failed',
          {
            privateBeachId: id,
            error: err instanceof Error ? err.message : String(err),
          },
          'warn',
        );
      }
    },
    [id, managerToken, managerSettings.managerUrl],
  );

  const addTiles = useCallback(
    (sessionIds: string[]) => {
      if (!id) return;
      const filteredIds = sessionIds.filter((value): value is string => typeof value === 'string' && value.trim().length > 0);
      if (filteredIds.length === 0) {
        return;
      }
      const sessionsById = new Map(sessions.map((session) => [session.session_id, session] as const));
      let nextLayout: CanvasLayout | null = null;
      setCanvasLayout((prev) => {
        const base = ensureLayoutMetadata(prev);
        const tiles = { ...base.tiles };
        let changed = false;
        let index = Object.keys(tiles).length;
        for (const sessionId of filteredIds) {
          if (tiles[sessionId]) {
            continue;
          }
          const position = computeAutoPosition(index);
          index += 1;
          const session = sessionsById.get(sessionId);
          tiles[sessionId] = {
            id: sessionId,
            kind: 'application',
            position,
            size: { width: DEFAULT_CANVAS_TILE_WIDTH, height: DEFAULT_CANVAS_TILE_HEIGHT },
            zIndex: index,
            zoom: 1,
            locked: false,
            toolbarPinned: false,
          };
          changed = true;
        }
        if (!changed) {
          return prev ?? base;
        }
        const next = withUpdatedTimestamp({
          ...base,
          tiles,
        });
        nextLayout = next;
        return next;
      });
      if (nextLayout) {
        void persistCanvasLayout(nextLayout);
      }
      for (const sessionId of filteredIds) {
        knownSessionIds.current.add(sessionId);
      }
    },
    [id, managerToken, managerSettings.managerUrl, sessions, persistCanvasLayout],
  );

  const refresh = useCallback(
    async (override?: RefreshOverride, metadata: RefreshMetadata = {}) => {
      if (!isLoaded || !isSignedIn) {
        debugLog(
          'dashboard',
          'refresh skipped (auth not ready)',
          {
            ...metadata,
            privateBeachId: id,
            isLoaded,
            isSignedIn,
          },
        );
        return;
      }
      const startedAt = Date.now();
      if (!id) return;
      setLoading(true);
      setError(null);
      const effectiveManagerUrl =
        override?.managerUrl && override.managerUrl.length > 0
          ? override.managerUrl
          : managerSettings.managerUrl;
      const stack = debugStack(1);
      debugLog('dashboard', 'refresh invoked', {
        ...metadata,
        privateBeachId: id,
        managerUrl: effectiveManagerUrl,
        overrideApplied: Boolean(override?.managerUrl),
        stack,
      });
      try {
        const token = await ensureManagerToken();
        if (!token) {
          setSessions([]);
          setError('Missing manager auth token');
          debugLog(
            'dashboard',
            'refresh aborted: missing manager token',
            {
              ...metadata,
              privateBeachId: id,
              managerUrl: effectiveManagerUrl,
            },
            'warn',
          );
          console.error('[dashboard] refresh aborted: missing manager token', {
            privateBeachId: id,
            managerUrl: effectiveManagerUrl,
          });
          return;
        }
        console.info('[dashboard] refreshing sessions', {
          privateBeachId: id,
          managerUrl: effectiveManagerUrl,
          token: token.slice(0, 4) + '…',
        });
        const data = await listSessions(id, token, effectiveManagerUrl);
        console.debug('[dashboard] sessions loaded', {
          count: data.length,
        });
        if (hasLoadedSessionsRef.current) {
          const previousKnown = knownSessionIds.current;
          const addedIds = data
            .filter((session) => !previousKnown.has(session.session_id))
            .map((session) => session.session_id);
          if (addedIds.length > 0) {
            addTiles(addedIds);
          }
        } else {
          hasLoadedSessionsRef.current = true;
        }
        knownSessionIds.current = new Set(data.map((session) => session.session_id));
        debugLog('dashboard', 'refresh succeeded', {
          ...metadata,
          privateBeachId: id,
          managerUrl: effectiveManagerUrl,
          sessionCount: data.length,
          durationMs: Date.now() - startedAt,
        });
        setSessions(data);
        try {
          const controllerIds = data.map((session) => session.session_id);
          const assignmentData = await listControllerPairingsForControllers(controllerIds, token, effectiveManagerUrl);
          setAssignments(assignmentData);
          debugLog(
            'dashboard',
            'assignments refresh succeeded',
            {
              ...metadata,
              privateBeachId: id,
              managerUrl: effectiveManagerUrl,
              assignments: assignmentData.length,
            },
          );
        } catch (pairingErr: any) {
          debugLog(
            'dashboard',
            'assignments refresh failed',
            {
              ...metadata,
              privateBeachId: id,
              managerUrl: effectiveManagerUrl,
              error: pairingErr?.message ?? String(pairingErr),
            },
            'warn',
          );
          console.error('[dashboard] listControllerPairingsForControllers failed', {
            privateBeachId: id,
            managerUrl: effectiveManagerUrl,
            error: pairingErr,
          });
        }
      } catch (e: any) {
        debugLog(
          'dashboard',
          'refresh failed',
          {
            ...metadata,
            privateBeachId: id,
            managerUrl: effectiveManagerUrl,
            error: e?.message ?? String(e),
          },
          'warn',
        );
        console.error('[dashboard] listSessions failed', {
          privateBeachId: id,
          managerUrl: effectiveManagerUrl,
          error: e,
        });
        setError(e.message || 'Failed to load sessions');
      } finally {
        setLoading(false);
        debugLog('dashboard', 'refresh finished', {
          ...metadata,
          privateBeachId: id,
          managerUrl: effectiveManagerUrl,
          durationMs: Date.now() - startedAt,
        });
      }
    },
    [id, isLoaded, isSignedIn, managerSettings.managerUrl, ensureManagerToken, addTiles],
  );

  const updateManagerSettings = useCallback(
    async (partial: Partial<ManagerSettings>) => {
      if (!id) {
        throw new Error('Beach id is not loaded yet');
      }
      const prevSettings = settingsJson;
      const nextManager: ManagerSettings = {
        managerUrl: partial.managerUrl !== undefined ? partial.managerUrl : managerSettings.managerUrl,
        roadUrl: partial.roadUrl !== undefined ? partial.roadUrl : managerSettings.roadUrl,
      };
      const nextSettings = {
        ...(prevSettings && typeof prevSettings === 'object' ? prevSettings : {}),
        manager: {
          manager_url: nextManager.managerUrl,
          road_url: nextManager.roadUrl,
        },
      };
      setSettingsJson(nextSettings);
      setSavingSettings(true);
      try {
        const token = await ensureManagerToken();
        if (!token) {
          throw new Error('Missing manager auth token');
        }
        await updateBeach(
          id,
          { settings: nextSettings },
          token,
          nextManager.managerUrl && nextManager.managerUrl.length > 0 ? nextManager.managerUrl : defaultManagerUrl,
        );
        await refresh(
          { managerUrl: nextManager.managerUrl },
          {
            source: 'updateManagerSettings',
            reason: 'manager-settings-updated',
            privateBeachId: id,
          },
        );
      } catch (err) {
        setSettingsJson(prevSettings);
        throw err;
      } finally {
        setSavingSettings(false);
      }
    },
    [id, settingsJson, managerSettings, defaultManagerUrl, refresh, ensureManagerToken],
  );

  const handleCanvasAssignmentsUpdated = useCallback(
    ({ agentId, targetIds, response }: { agentId: string; targetIds: string[]; response: BatchAssignmentResponse }) => {
      const successes = response.results.filter((result) => result.ok);
      if (successes.length === 0) {
        return;
      }
      const pairings = response.results
        .map((result) => result.pairing)
        .filter((pairing): pairing is ControllerPairing => Boolean(pairing));
      if (pairings.length > 0) {
        setAssignments((prev) => {
          const map = new Map(
            prev.map((entry) => [`${entry.controller_session_id}|${entry.child_session_id}`, entry] as const),
          );
          for (const pairing of pairings) {
            map.set(`${pairing.controller_session_id}|${pairing.child_session_id}`, pairing);
          }
          return Array.from(map.values());
        });
        return;
      }
      void refresh(undefined, {
        source: 'canvas-surface',
        reason: 'assignment-drop',
        controllerId: agentId,
        count: successes.length,
        targetIds,
      }).catch(() => {});
    },
    [refresh],
  );

  const refreshRef = useRef(refresh);
  useEffect(() => {
    refreshRef.current = refresh;
  }, [refresh]);

  const lastAutoRefreshKey = useRef<string | null>(null);
  useEffect(() => {
    if (!id) {
      return;
    }
    if (!isLoaded || !isSignedIn) {
      debugLog(
        'dashboard',
        'auto refresh waiting for auth',
        {
          privateBeachId: id,
          isLoaded,
          isSignedIn,
        },
      );
      return;
    }
    const key = `${id}|${managerSettings.managerUrl}`;
    const previousKey = lastAutoRefreshKey.current;
    if (previousKey === key) {
      debugLog('dashboard', 'auto refresh skipped (unchanged key)', {
        privateBeachId: id,
        managerUrl: managerSettings.managerUrl,
        key,
      });
      return;
    }
    lastAutoRefreshKey.current = key;
    const reason = previousKey ? 'id-or-manager-url-changed' : 'initial-load';
    debugLog('dashboard', 'auto refresh triggered', {
      privateBeachId: id,
      managerUrl: managerSettings.managerUrl,
      key,
      previousKey,
      reason,
    });
    refreshRef
      .current(undefined, { source: 'auto-effect', reason })
      .catch(() => {});
  }, [id, isLoaded, isSignedIn, managerSettings.managerUrl]);

  const lastViewerRefreshKey = useRef<string | null>(null);
  useEffect(() => {
    if (!id || !viewerToken || !isLoaded || !isSignedIn) {
      return;
    }
    const key = `${id}|${viewerToken}`;
    if (lastViewerRefreshKey.current === key) {
      debugLog('dashboard', 'token-ready refresh skipped (unchanged key)', {
        privateBeachId: id,
        key,
      });
      return;
    }
    lastViewerRefreshKey.current = key;
    debugLog('dashboard', 'token-ready refresh scheduled', {
      privateBeachId: id,
    });
    refreshRef
      .current(undefined, { source: 'token-ready', reason: 'manager-token-ready' })
      .catch(() => {});
  }, [id, viewerToken, isLoaded, isSignedIn]);

  const addTile = useCallback(
    (sessionId: string) => {
      addTiles([sessionId]);
    },
    [addTiles],
  );

  const removeTile = useCallback(
    (sessionId: string) => {
      if (!id) return;
      let nextLayout: CanvasLayout | null = null;
      setCanvasLayout((prev) => {
        const base = ensureLayoutMetadata(prev);
        if (!base.tiles[sessionId]) {
          return prev ?? base;
        }
        const { [sessionId]: _omit, ...rest } = base.tiles;
        const next = withUpdatedTimestamp({
          ...base,
          tiles: rest,
        });
        nextLayout = next;
        return next;
      });
      if (nextLayout) {
        void persistCanvasLayout(nextLayout);
      }
    },
    [id, persistCanvasLayout, managerToken],
  );

  const sessionById = useMemo(() => new Map(sessions.map((s) => [s.session_id, s])), [sessions]);
  const assignmentModel = useMemo(
    () => buildAssignmentModel(sessions, assignments),
    [sessions, assignments],
  );
  const agentSessions = assignmentModel.agents;
  const applicationSessions = assignmentModel.applications;
  const assignmentsByAgent = assignmentModel.assignmentsByAgent;
  const assignmentsByApplication = assignmentModel.assignmentsByApplication;
  const sessionRoles = assignmentModel.roles;

  const tileSessions = useMemo(() => {
    if (!canvasLayout) return [] as SessionSummary[];
    const order = Object.keys(canvasLayout.tiles);
    const map = new Map(sessions.map((session) => [session.session_id, session] as const));
    return order
      .map((sessionId) => map.get(sessionId))
      .filter((entry): entry is SessionSummary => Boolean(entry));
  }, [canvasLayout, sessions]);
  const lastTileOrderRef = useRef<string | null>(null);
  useEffect(() => {
    const order = tileSessions.map((session) => session.session_id);
    const signature = order.join(',');
    if (typeof window !== 'undefined' && lastTileOrderRef.current !== signature) {
      console.info('[dashboard] tile-order', {
        order,
        count: order.length,
        previous: lastTileOrderRef.current,
      });
    }
    lastTileOrderRef.current = signature;
  }, [tileSessions]);

  const controllerSessionIds = useMemo(
    () => agentSessions.map((agent) => agent.session_id),
    [agentSessions],
  );

  useControllerPairingStreams({
    managerUrl,
    managerToken,
    controllerSessionIds,
    setAssignments,
  });

  useEffect(() => {
    if (selectedAgentId && !agentSessions.some((agent) => agent.session_id === selectedAgentId)) {
      setSelectedAgentId(null);
    }
  }, [agentSessions, selectedAgentId]);

  useEffect(() => {
    if (
      selectedApplicationId &&
      !applicationSessions.some((app) => app.session_id === selectedApplicationId)
    ) {
      setSelectedApplicationId(null);
    }
  }, [applicationSessions, selectedApplicationId]);

  function onSelect(s: SessionSummary) {
    setSelected(s);
    setDrawerOpen(true);
  }

  const handleSessionMetadataUpdate = useCallback(
    (sessionId: string, metadata: Record<string, unknown>) => {
      setSessions((prev) =>
        prev.map((existing) =>
          existing.session_id === sessionId ? { ...existing, metadata } : existing,
        ),
      );
      setSelected((prev) =>
        prev && prev.session_id === sessionId ? { ...prev, metadata } : prev,
      );
    },
    [setSessions, setSelected],
  );

  const settingsContextValue = useMemo(
    () => ({
      manager: managerSettings,
      updateManager: updateManagerSettings,
      saving: savingSettings,
    }),
    [managerSettings, updateManagerSettings, savingSettings],
  );

  const handleRoleChange = useCallback(
    async (session: SessionSummary, targetRole: SessionRole) => {
      if (!managerToken || managerToken.trim().length === 0) {
        setError('Missing manager auth token.');
        return;
      }
      setAssignmentError(null);
      try {
        if (targetRole === 'application') {
          const existing = assignmentsByAgent.get(session.session_id) ?? [];
          for (const edge of existing) {
            await deleteControllerPairing(
              session.session_id,
              edge.pairing.child_session_id,
              managerToken,
              managerUrl,
            );
          }
          setAssignments((prev) =>
            prev.filter(
              (entry) => entry.controller_session_id !== session.session_id,
            ),
          );
        }
        await updateSessionRole(session, targetRole, managerToken, managerUrl);
        setSessions((prev) =>
          prev.map((existing) =>
            existing.session_id === session.session_id
              ? { ...existing, metadata: buildMetadataWithRole(existing.metadata, targetRole) }
              : existing,
          ),
        );
        void refresh(undefined, {
          source: 'role-change',
          sessionId: session.session_id,
          role: targetRole,
        }).catch(() => {});
      } catch (err: any) {
        setError(err?.message ?? 'Failed to update session role');
      }
    },
    [managerToken, managerUrl, assignmentsByAgent, refresh],
  );

  const handleCreateAssignment = useCallback(
    async (agentId: string, applicationId: string) => {
      if (!managerToken || managerToken.trim().length === 0) {
        setAssignmentError('Missing manager auth token.');
        return;
      }
      setAssignmentSaving(true);
      setAssignmentError(null);
      try {
        const created = await createControllerPairing(
          agentId,
          {
            child_session_id: applicationId,
            prompt_template: null,
            update_cadence: 'balanced',
          },
          managerToken,
          managerUrl,
        );
        setAssignments((prev) => {
          const map = new Map(
            prev.map((entry) => [`${entry.controller_session_id}|${entry.child_session_id}`, entry]),
          );
          map.set(`${created.controller_session_id}|${created.child_session_id}`, created);
          return Array.from(map.values());
        });
        setSelectedAgentId(agentId);
        setSelectedApplicationId(applicationId);
        setActiveAssignment(created);
        setAssignmentPaneOpen(true);
        void refresh(undefined, {
          source: 'assignment',
          reason: 'created',
          controllerId: agentId,
          childId: applicationId,
        }).catch(() => {});
      } catch (err: any) {
        setAssignmentError(formatAssignmentError(err?.message ?? String(err)));
      } finally {
        setAssignmentSaving(false);
      }
    },
    [managerToken, managerUrl, refresh, formatAssignmentError],
  );

  const handleSaveAssignment = useCallback(
    async ({
      controllerId,
      childId,
      promptTemplate,
      updateCadence,
    }: {
      controllerId: string;
      childId: string;
      promptTemplate: string;
      updateCadence: ControllerUpdateCadence;
    }) => {
      if (!managerToken || managerToken.trim().length === 0) {
        setAssignmentError('Missing manager auth token.');
        return;
      }
      setAssignmentSaving(true);
      setAssignmentError(null);
      try {
        const updated = await createControllerPairing(
          controllerId,
          {
            child_session_id: childId,
            prompt_template: promptTemplate.trim().length > 0 ? promptTemplate : null,
            update_cadence: updateCadence,
          },
          managerToken,
          managerUrl,
        );
        setAssignments((prev) =>
          prev.map((entry) =>
            entry.controller_session_id === controllerId && entry.child_session_id === childId
              ? updated
              : entry,
          ),
        );
        setActiveAssignment(updated);
        void refresh(undefined, {
          source: 'assignment',
          reason: 'updated',
          controllerId,
          childId,
        }).catch(() => {});
      } catch (err: any) {
        setAssignmentError(formatAssignmentError(err?.message ?? String(err)));
      } finally {
        setAssignmentSaving(false);
      }
    },
    [managerToken, managerUrl, refresh, formatAssignmentError],
  );

  const handleRemoveAssignment = useCallback(
    async ({ controllerId, childId }: { controllerId: string; childId: string }) => {
      if (!managerToken || managerToken.trim().length === 0) {
        setAssignmentError('Missing manager auth token.');
        return;
      }
      setAssignmentSaving(true);
      setAssignmentError(null);
      try {
        await deleteControllerPairing(controllerId, childId, managerToken, managerUrl);
        setAssignments((prev) =>
          prev.filter(
            (entry) =>
              !(entry.controller_session_id === controllerId && entry.child_session_id === childId),
          ),
        );
        setAssignmentPaneOpen(false);
        setActiveAssignment(null);
        void refresh(undefined, {
          source: 'assignment',
          reason: 'deleted',
          controllerId,
          childId,
        }).catch(() => {});
      } catch (err: any) {
        setAssignmentError(formatAssignmentError(err?.message ?? String(err)));
      } finally {
        setAssignmentSaving(false);
      }
    },
    [managerToken, managerUrl, refresh, formatAssignmentError],
  );

  const handleOpenAssignment = useCallback((pairing: ControllerPairing) => {
    setActiveAssignment(pairing);
    setAssignmentPaneOpen(true);
    setSelectedAgentId(pairing.controller_session_id);
    setSelectedApplicationId(pairing.child_session_id);
    setAssignmentError(null);
  }, []);

  const activeAssignmentController = useMemo(() => {
    if (!activeAssignment) return null;
    return sessionById.get(activeAssignment.controller_session_id) ?? null;
  }, [activeAssignment, sessionById]);

  const activeAssignmentChild = useMemo(() => {
    if (!activeAssignment) return null;
    return sessionById.get(activeAssignment.child_session_id) ?? null;
  }, [activeAssignment, sessionById]);

  return (
    <BeachSettingsProvider value={settingsContextValue}>
      <div className="flex min-h-screen flex-col">
        <TopNav
          currentId={id}
          onSwitch={(v) => router.push(`/beaches/${v}`)}
          right={
            <div className="flex items-center gap-2">
              <Button
                variant={sidebarOpen ? 'default' : 'outline'}
                onClick={() => setSidebarOpen((prev) => !prev)}
                aria-pressed={sidebarOpen}
              >
                {sidebarOpen ? 'Hide Explorer' : 'Show Explorer'}
              </Button>
              <Button variant="outline" onClick={() => setAddOpen(true)}>Add</Button>
              <Button
                onClick={() => {
                  refresh(undefined, { source: 'top-nav', reason: 'manual-refresh' }).catch(() => {});
                }}
                disabled={loading}
              >
                {loading ? 'Loading…' : 'Refresh'}
              </Button>
              <BeachSettingsButton />
            </div>
          }
        />
        <div className="flex flex-1 min-h-0 flex-col md:flex-row">
          {sidebarOpen && (
            <div className="w-full md:w-[320px] md:flex-none">
              <AgentExplorer
                agents={agentSessions}
                applications={applicationSessions}
                assignmentsByAgent={assignmentsByAgent}
                assignmentsByApplication={assignmentsByApplication}
                onCreateAssignment={handleCreateAssignment}
                onRemoveAssignment={(agentId, applicationId) =>
                  handleRemoveAssignment({ controllerId: agentId, childId: applicationId })
                }
                onOpenAssignment={handleOpenAssignment}
                selectedAgentId={selectedAgentId}
                onSelectAgent={setSelectedAgentId}
                selectedApplicationId={selectedApplicationId}
                onSelectApplication={setSelectedApplicationId}
                onAddToLayout={addTile}
              />
            </div>
          )}
          <div className="flex-1 min-w-0 flex flex-col min-h-0">
            {error && <div className="mb-2 rounded-md border border-red-500/40 bg-red-500/10 p-2 text-sm text-red-600 dark:text-red-400">{error}</div>}
            {assignmentError && !assignmentPaneOpen && (
              <div className="mb-2 rounded-md border border-amber-500/40 bg-amber-500/10 p-2 text-sm text-amber-700 dark:text-amber-300">
                {assignmentError}
              </div>
            )}
            <div className="flex-1 min-h-0">
              <CanvasSurface
                tiles={tileSessions}
                agents={agentSessions}
                layout={canvasLayout}
                onLayoutChange={handleCanvasLayoutChange}
                onPersistLayout={persistCanvasLayout}
                onRemove={removeTile}
                onSelect={onSelect}
                privateBeachId={id ?? null}
                managerToken={managerToken}
                managerUrl={managerUrl}
                viewerToken={viewerToken}
                handlers={{
                  onAssignAgent: handleCanvasAssignmentsUpdated,
                  onAssignmentError: handleCanvasAssignmentError,
                }}
              />
            </div>
          </div>
        </div>
        <SessionDrawer
          open={drawerOpen}
          onOpenChange={setDrawerOpen}
          session={selected}
          managerUrl={managerUrl}
          token={managerToken}
          onSessionMetadataUpdate={handleSessionMetadataUpdate}
        />
        {id && (
          <AddSessionModal
            open={addOpen}
            onOpenChange={setAddOpen}
            privateBeachId={id}
            managerUrl={managerUrl}
            roadUrl={roadUrl}
            token={managerToken}
            onAttached={(sessionIds) =>
              {
                addTiles(sessionIds);
                refresh(undefined, {
                  source: 'add-session-modal',
                  reason: 'session-attached',
                  sessionIds,
                }).catch(() => {});
              }
            }
          />
        )}
        <AssignmentDetailPane
          open={assignmentPaneOpen && Boolean(activeAssignment && activeAssignmentController && activeAssignmentChild)}
          pairing={assignmentPaneOpen ? activeAssignment : null}
          controller={activeAssignmentController}
          child={activeAssignmentChild}
          onClose={() => {
            setAssignmentPaneOpen(false);
            setAssignmentError(null);
          }}
          onSave={handleSaveAssignment}
          onRemove={handleRemoveAssignment}
          saving={assignmentSaving}
          error={assignmentError}
        />
      </div>
    </BeachSettingsProvider>
  );
}
