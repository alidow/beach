import { useRouter } from 'next/router';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useAuth } from '@clerk/nextjs';
import TopNav from '../../../components/TopNav';
import SessionListPanel from '../../../components/SessionListPanel';
import TileCanvas from '../../../components/TileCanvas';
import SessionDrawer from '../../../components/SessionDrawer';
import { Button } from '../../../components/ui/button';
import AddSessionModal from '../../../components/AddSessionModal';
import { Select } from '../../../components/ui/select';
import { SessionSummary, listSessions, getBeachMeta, getBeachLayout, putBeachLayout, updateBeach } from '../../../lib/api';
import type { BeachLayout } from '../../../lib/api';
import { BeachSettingsProvider, ManagerSettings } from '../../../components/settings/BeachSettingsContext';
import { BeachSettingsButton } from '../../../components/settings/SettingsButton';
import { debugLog, debugStack } from '../../../lib/debug';

export default function BeachDashboard() {
  const router = useRouter();
  const { id } = router.query as { id?: string };
  const [beachName, setBeachName] = useState<string>('');
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [layout, setLayout] = useState<BeachLayout>({ tiles: [], preset: 'grid2x2', layout: [] });
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [selected, setSelected] = useState<SessionSummary | null>(null);
  const [addOpen, setAddOpen] = useState(false);
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
      if (token && token.trim().length > 0) {
        setViewerToken((prev) => {
          if (prev && prev.trim().length > 0) {
            return prev;
          }
          return token;
        });
      } else {
        setViewerToken(null);
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
          return;
        }
        debugLog(
          'dashboard',
          'meta fetch token acquired',
          {
            privateBeachId: id,
          },
        );
        const meta = await getBeachMeta(id, token);
        if (!active) return;
        setBeachName(meta.name);
        setSettingsJson(meta.settings || {});
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
      }
      try {
        const l = await getBeachLayout(id, managerToken);
        if (active) setLayout(l);
      } catch {
        // ignore, keep default
      }
    })();
    return () => {
      active = false;
    };
  }, [id, isLoaded, isSignedIn, ensureManagerToken, managerToken]);

  type RefreshOverride = { managerUrl?: string };
  type RefreshMetadata = Record<string, unknown>;

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
        debugLog('dashboard', 'refresh succeeded', {
          ...metadata,
          privateBeachId: id,
          managerUrl: effectiveManagerUrl,
          sessionCount: data.length,
          durationMs: Date.now() - startedAt,
        });
        setSessions(data);
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
    [id, isLoaded, isSignedIn, managerSettings.managerUrl, ensureManagerToken],
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

  const persistLayoutSnapshot = useCallback(
    (items: BeachLayout['layout']) => {
      if (!id) return;
      setLayout((prev) => {
        const stack = debugStack(1);
        debugLog('layout', 'persist invoked', {
          privateBeachId: id,
          incomingCount: items.length,
          stack,
        });
        const allowed = new Set(prev.tiles);
        const filtered = items.filter((entry) => allowed.has(entry.id));
        const unchanged =
          filtered.length === prev.layout.length &&
          filtered.every((entry, index) => {
            const existing = prev.layout[index];
            return (
              !!existing &&
              existing.id === entry.id &&
              existing.x === entry.x &&
              existing.y === entry.y &&
              existing.w === entry.w &&
              existing.h === entry.h
            );
          });
        if (unchanged) {
          debugLog('layout', 'persist skipped (unchanged)', {
            privateBeachId: id,
            tilesTracked: prev.tiles.length,
            stack,
          });
          return prev;
        }
        const next = { ...prev, layout: filtered };
        debugLog('layout', 'persist scheduled', {
          privateBeachId: id,
          tilesTracked: prev.tiles.length,
          persistedCount: filtered.length,
          layout: filtered.map(({ id: layoutId, x, y, w, h }) => ({ id: layoutId, x, y, w, h })),
          stack,
        });
        void putBeachLayout(id, next, managerToken).catch(() => {});
        return next;
      });
    },
    [id, managerToken],
  );

  function addTile(sessionId: string) {
    if (!id) return;
    const tiles = Array.from(new Set([sessionId, ...layout.tiles])).slice(0, 6);
    const allowed = new Set(tiles);
    const next = { ...layout, tiles, layout: layout.layout.filter((entry) => allowed.has(entry.id)) };
    setLayout(next);
    putBeachLayout(id, next, managerToken).catch(() => {});
  }
  function removeTile(sessionId: string) {
    if (!id) return;
    const tiles = layout.tiles.filter((t) => t !== sessionId);
    const allowed = new Set(tiles);
    const next = { ...layout, tiles, layout: layout.layout.filter((entry) => allowed.has(entry.id)) };
    setLayout(next);
    putBeachLayout(id, next, managerToken).catch(() => {});
  }
  function changePreset(preset: BeachLayout['preset']) {
    if (!id) return;
    const next = { ...layout, preset, layout: [] };
    setLayout(next);
    putBeachLayout(id, next, managerToken).catch(() => {});
  }

  const tileSessions = useMemo(() => {
    const byId = new Map(sessions.map((s) => [s.session_id, s] as const));
    return layout.tiles.map((id) => byId.get(id)).filter(Boolean) as SessionSummary[];
  }, [sessions, layout.tiles]);

  function onSelect(s: SessionSummary) {
    setSelected(s);
    setDrawerOpen(true);
  }

  const settingsContextValue = useMemo(
    () => ({
      manager: managerSettings,
      updateManager: updateManagerSettings,
      saving: savingSettings,
    }),
    [managerSettings, updateManagerSettings, savingSettings],
  );

  return (
    <BeachSettingsProvider value={settingsContextValue}>
      <div className="min-h-screen">
        <TopNav
          currentId={id}
          onSwitch={(v) => router.push(`/beaches/${v}`)}
          right={
            <div className="flex items-center gap-2">
              <Select
                value={layout.preset}
                onChange={(v) => changePreset(v as any)}
                options={[
                  { value: 'grid2x2', label: 'Layout: Grid' },
                  { value: 'onePlusThree', label: 'Layout: 1+3' },
                  { value: 'focus', label: 'Layout: Focus' },
                ]}
              />
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
        <div className="grid grid-cols-12 gap-3 p-3">
          <div className="col-span-12 md:col-span-3">
            <div className="h-[calc(100vh-4rem)] rounded-lg border border-border bg-card text-card-foreground shadow-sm">
              <SessionListPanel sessions={sessions} onAdd={addTile} />
            </div>
          </div>
          <div className="col-span-12 md:col-span-9">
            {error && <div className="mb-2 rounded-md border border-red-500/40 bg-red-500/10 p-2 text-sm text-red-600 dark:text-red-400">{error}</div>}
              <TileCanvas
                tiles={tileSessions}
                onRemove={removeTile}
                onSelect={onSelect}
                managerToken={managerToken}
                viewerToken={viewerToken}
                managerUrl={managerUrl}
                refresh={(metadata) => refresh(undefined, { source: 'tile-canvas', ...(metadata ?? {}) })}
                preset={layout.preset}
                savedLayout={layout.layout}
                onLayoutPersist={persistLayoutSnapshot}
              />
          </div>
        </div>
        <SessionDrawer open={drawerOpen} onOpenChange={setDrawerOpen} session={selected} managerUrl={managerUrl} token={managerToken} />
        {id && (
          <AddSessionModal
            open={addOpen}
            onOpenChange={setAddOpen}
            privateBeachId={id}
            managerUrl={managerUrl}
            roadUrl={roadUrl}
            token={managerToken}
            onAttached={(sessionIds) =>
              refresh(undefined, {
                source: 'add-session-modal',
                reason: 'session-attached',
                sessionIds,
              }).catch(() => {})
            }
          />
        )}
      </div>
    </BeachSettingsProvider>
  );
}
