import { Buffer } from 'buffer';
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
import PongShowcaseDrawer from '../../../components/PongShowcaseDrawer';
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
  acquireController,
  buildMetadataWithRole,
  type SessionRole,
  type ControllerPairing,
  revokeControllerHandshake,
} from '../../../lib/api';
import type { CanvasLayout, ControllerUpdateCadence } from '../../../lib/api';
import type { BatchAssignmentResponse } from '../../../canvas';
import { BeachSettingsProvider, ManagerSettings } from '../../../components/settings/BeachSettingsContext';
import { BeachSettingsButton } from '../../../components/settings/SettingsButton';
import { AgentExplorer } from '../../../components/AgentExplorer';
import { AssignmentDetailPane } from '../../../components/AssignmentDetailPane';
import { debugLog, debugStack } from '../../../lib/debug';
import { emitTelemetry } from '../../../lib/telemetry';
import { isPrivateBeachRewriteEnabled, resolvePrivateBeachRewriteEnabled } from '../../../lib/featureFlags';
import { useControllerPairingStreams } from '../../../hooks/useControllerPairingStreams';
import { useManagerToken } from '../../../hooks/useManagerToken';
import { buildAssignmentModel } from '../../../lib/assignments';

const DEFAULT_CANVAS_TILE_WIDTH = 448;
const DEFAULT_CANVAS_TILE_HEIGHT = 448;
const DEFAULT_CANVAS_TILE_GAP = 32;
const AUTO_LAYOUT_COLUMNS = 4;

type JwtClaims = {
  scope?: string;
  scp?: string[];
  entitlements?: string[];
  [key: string]: unknown;
};

function base64UrlDecode(segment: string): string | null {
  if (!segment) return null;
  const normalized = segment.replace(/-/g, '+').replace(/_/g, '/');
  const padding = normalized.length % 4 === 0 ? '' : '='.repeat(4 - (normalized.length % 4));
  const payload = normalized + padding;
  try {
    if (typeof window !== 'undefined' && typeof window.atob === 'function') {
      return window.atob(payload);
    }
    // Buffer exists in Node/SSR contexts.
    return Buffer.from(payload, 'base64').toString('utf-8');
  } catch (err) {
    console.warn('[auth] failed to base64 decode token segment', err);
    return null;
  }
}

function decodeJwtClaims(token: string | null): JwtClaims | null {
  if (!token) return null;
  const parts = token.split('.');
  if (parts.length < 2) {
    return null;
  }
  const decoded = base64UrlDecode(parts[1]);
  if (!decoded) {
    return null;
  }
  try {
    return JSON.parse(decoded);
  } catch (err) {
    console.warn('[auth] failed to parse token payload JSON', err);
    return null;
  }
}

const TOKEN_REFRESH_SKEW_MS = 30_000;

function getTokenExpiryMs(token: string | null): number | null {
  if (!token || token.trim().length === 0) {
    return null;
  }
  const claims = decodeJwtClaims(token);
  const expSeconds = (claims as Record<string, unknown>)?.exp;
  if (typeof expSeconds !== 'number') {
    return null;
  }
  return expSeconds * 1000;
}

function isTokenExpiring(token: string | null, skewMs = TOKEN_REFRESH_SKEW_MS) {
  const expiryMs = getTokenExpiryMs(token);
  if (!expiryMs) {
    return false;
  }
  return Date.now() >= expiryMs - skewMs;
}

function logTokenClaims(token: string | null, context: Record<string, unknown>) {
  const payload: Record<string, unknown> = {
    ...context,
    hasToken: Boolean(token && token.trim().length > 0),
  };
  if (token && token.trim().length > 0) {
    const claims = decodeJwtClaims(token) ?? {};
    const entitlements =
      (claims as Record<string, unknown>)?.entitlements ??
      (claims as Record<string, unknown>)?.entitlememts ??
      null;
    payload.scope = (claims as Record<string, unknown>)?.scope ?? null;
    payload.scp = (claims as Record<string, unknown>)?.scp ?? null;
    payload.entitlements = entitlements;
    payload.exp = getTokenExpiryMs(token);
    payload.tokenPrefix = token.slice(0, 12);
  }
  try {
    console.info('[auth] token-claims', JSON.stringify(payload));
  } catch {
    console.info('[auth] token-claims', payload);
  }
}

type RemovedTileInfo = {
  sessionId: string;
  kind?: string;
  position?: { x: number; y: number };
};

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
  const [showcaseOpen, setShowcaseOpen] = useState(false);
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
  const { isLoaded, isSignedIn } = useAuth();
  const [managerToken, setManagerToken] = useState<string | null>(null);
  const [viewerToken, setViewerToken] = useState<string | null>(null);
  const [assignments, setAssignments] = useState<ControllerPairing[]>([]);
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const [selectedApplicationId, setSelectedApplicationId] = useState<string | null>(null);
  const [activeAssignment, setActiveAssignment] = useState<ControllerPairing | null>(null);
  const [assignmentPaneOpen, setAssignmentPaneOpen] = useState(false);
  const [assignmentSaving, setAssignmentSaving] = useState(false);
  const [assignmentError, setAssignmentError] = useState<string | null>(null);
  const [rewriteEnabled, setRewriteEnabled] = useState(() => resolvePrivateBeachRewriteEnabled());
  const { token: resolvedManagerToken, refresh: refreshManagerToken } = useManagerToken(isLoaded && isSignedIn);
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
  const handshakeTokensRef = useRef<Map<string, string>>(new Map());

  useEffect(() => {
    canvasLayoutRef.current = canvasLayout;
  }, [canvasLayout]);

  useEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }
    setRewriteEnabled(isPrivateBeachRewriteEnabled());
  }, []);

  useEffect(() => {
    emitTelemetry('canvas.rewrite.flag-state', {
      privateBeachId: id ?? null,
      enabled: rewriteEnabled,
    });
  }, [id, rewriteEnabled]);

  useEffect(() => {
    if (!isLoaded || !isSignedIn) {
      setManagerToken(null);
      setViewerToken(null);
      return;
    }
    if (!resolvedManagerToken || resolvedManagerToken.trim().length === 0) {
      setManagerToken(null);
      setViewerToken(null);
      return;
    }
    setManagerToken(resolvedManagerToken);
    setViewerToken((prev) => {
      const reuseExisting = Boolean(prev && prev.trim().length > 0);
      return reuseExisting ? prev : resolvedManagerToken;
    });
    logTokenClaims(resolvedManagerToken, { phase: 'hook-refresh' });
  }, [isLoaded, isSignedIn, resolvedManagerToken]);

  const ensureManagerToken = useCallback(async () => {
    if (managerToken && managerToken.trim().length > 0) {
      if (!isTokenExpiring(managerToken)) {
        logTokenClaims(managerToken, { phase: 'ensure-cache-hit' });
        return managerToken;
      }
      debugLog('auth', 'manager token expiring; refreshing', {
        phase: 'ensure-refresh',
      });
    }
    const refreshed = await refreshManagerToken({ force: true });
    if (refreshed && refreshed.trim().length > 0) {
      logTokenClaims(refreshed, { phase: 'ensure-refresh-success' });
      setManagerToken(refreshed);
      setViewerToken((prev) => {
        const reuseExisting = Boolean(prev && prev.trim().length > 0);
        return reuseExisting ? prev : refreshed;
      });
      return refreshed;
    }
    setViewerToken(null);
    return null;
  }, [managerToken, refreshManagerToken]);

  useEffect(() => {
    if (!isLoaded) return;
    if (!isSignedIn) {
      router.replace('/sign-in');
    }
  }, [isLoaded, isSignedIn, router]);

  useEffect(() => {
    if (!isLoaded || !isSignedIn) {
      setManagerToken(null);
      setViewerToken(null);
    }
  }, [isLoaded, isSignedIn]);

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
       const createdTiles: {
         sessionId: string;
         position: { x: number; y: number };
         order: number;
         sessionName: string | null;
       }[] = [];
      setCanvasLayout((prev) => {
        const base = ensureLayoutMetadata(prev);
        const tiles = { ...base.tiles };
        let changed = false;
        let index = Object.keys(tiles).length;
        for (const sessionId of filteredIds) {
          if (tiles[sessionId]) {
            continue;
          }
          const placementOrder = index;
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
          const legacyName =
            (session as SessionSummary & { display_name?: string | null })?.display_name ?? null;
          createdTiles.push({
            sessionId,
            position,
            order: placementOrder,
            sessionName: legacyName ?? session?.metadata?.name ?? null,
          });
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
      for (const entry of createdTiles) {
        emitTelemetry('canvas.tile.create', {
          privateBeachId: id,
          sessionId: entry.sessionId,
          position: entry.position,
          order: entry.order,
          rewriteEnabled,
          sessionName: entry.sessionName,
        });
      }
      for (const sessionId of filteredIds) {
        knownSessionIds.current.add(sessionId);
      }
    },
    [id, sessions, persistCanvasLayout, rewriteEnabled],
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
      // Revoke controller lease if we previously issued a handshake for this session
      const token = handshakeTokensRef.current.get(sessionId);
      if (token) {
        const doRevoke = async () => {
          try {
            const managerToken = await ensureManagerToken();
            if (!managerToken) return;
            await revokeControllerHandshake(sessionId, token, managerToken, managerUrl);
          } catch (err) {
            console.warn('[dashboard] revokeControllerHandshake failed', { sessionId, err });
          } finally {
            handshakeTokensRef.current.delete(sessionId);
          }
        };
        void doRevoke();
      }
      // Placeholder: lease revocation managed by handshake-token map below (if available)
      let nextLayout: CanvasLayout | null = null;
      let removedTile: RemovedTileInfo | null = null;
      let remainingTileCount = 0;
      setCanvasLayout((prev) => {
        const base = ensureLayoutMetadata(prev);
        const existing = base.tiles[sessionId];
        if (!existing) {
          return prev ?? base;
        }
        removedTile = {
          sessionId,
          kind: existing.kind,
          position: existing.position,
        };
        const { [sessionId]: _omit, ...rest } = base.tiles;
        const next = withUpdatedTimestamp({
          ...base,
          tiles: rest,
        });
        nextLayout = next;
        remainingTileCount = Object.keys(next.tiles).length;
        return next;
      });
      if (nextLayout) {
        void persistCanvasLayout(nextLayout);
      }
      const removedTileInfo = removedTile;
      if (removedTileInfo) {
        const info = removedTileInfo as RemovedTileInfo;
        emitTelemetry('canvas.tile.remove', {
          privateBeachId: id,
          sessionId: info.sessionId,
          kind: info.kind ?? 'application',
          position: info.position ?? null,
          remainingTiles: remainingTileCount,
          rewriteEnabled,
        });
      }
    },
    [ensureManagerToken, id, managerUrl, persistCanvasLayout, rewriteEnabled],
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
        // Manager requires the controller session to hold a lease before pairing
        try {
          await acquireController(agentId, 30_000, managerToken, managerUrl);
        } catch (leaseErr: any) {
          console.warn('[assignments] acquireController (create) failed', {
            controller: agentId,
            error: leaseErr?.message ?? String(leaseErr),
          });
        }
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
        try {
          await acquireController(controllerId, 30_000, managerToken, managerUrl);
        } catch (leaseErr: any) {
          console.warn('[assignments] acquireController (update) failed', {
            controller: controllerId,
            error: leaseErr?.message ?? String(leaseErr),
          });
        }
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
        try {
          await acquireController(controllerId, 30_000, managerToken, managerUrl);
        } catch (leaseErr: any) {
          console.warn('[assignments] acquireController (delete) failed', {
            controller: controllerId,
            error: leaseErr?.message ?? String(leaseErr),
          });
        }
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
      <div className="flex min-h-screen flex-col" data-rewrite-enabled={rewriteEnabled ? 'true' : 'false'}>
        <TopNav
          currentId={id}
          onSwitch={(v) => router.push(`/beaches/${v}`)}
          right={
            <div className="flex items-center gap-2">
              <Button
                variant={sidebarOpen ? 'primary' : 'outline'}
                onClick={() => setSidebarOpen((prev) => !prev)}
                aria-pressed={sidebarOpen}
              >
                {sidebarOpen ? 'Hide Explorer' : 'Show Explorer'}
              </Button>
              <Button variant="outline" onClick={() => setShowcaseOpen(true)}>
                Showcase
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
            onHandshakeIssued={(sessionId, controllerToken) => {
              handshakeTokensRef.current.set(sessionId, controllerToken);
            }}
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
        <PongShowcaseDrawer
          open={showcaseOpen}
          onOpenChange={setShowcaseOpen}
          privateBeachId={id ?? ''}
          token={managerToken}
          managerUrl={managerUrl}
        />
      </div>
    </BeachSettingsProvider>
  );
}
