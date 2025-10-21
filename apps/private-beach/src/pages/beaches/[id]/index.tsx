import { useRouter } from 'next/router';
import { useCallback, useEffect, useMemo, useState } from 'react';
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
      setManagerToken(null);
      return null;
    }
    const token = await getToken(
      tokenTemplate ? { template: tokenTemplate } : undefined,
    );
    setManagerToken(token ?? null);
    return token ?? null;
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
      return;
    }
    const interval = setInterval(() => {
      refreshManagerToken().catch(() => {});
    }, 60_000);
    return () => clearInterval(interval);
  }, [isLoaded, isSignedIn, refreshManagerToken]);

  useEffect(() => {
    if (!id) return;
    let active = true;
    (async () => {
      try {
        const token = await ensureManagerToken();
        if (!token || !active) {
          setError('Not authorized to load beach.');
          return;
        }
        const meta = await getBeachMeta(id, token);
        if (!active) return;
        setBeachName(meta.name);
        setSettingsJson(meta.settings || {});
      } catch (e: any) {
        if (!active) return;
        if (e?.message === 'not_found') setError('Beach not found'); else setError('Failed to load beach');
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
  }, [id, ensureManagerToken, managerToken]);

  type RefreshOverride = { managerUrl?: string };

  const refresh = useCallback(
    async (override?: RefreshOverride) => {
      if (!id) return;
      setLoading(true);
      setError(null);
      const effectiveManagerUrl =
        override?.managerUrl && override.managerUrl.length > 0
          ? override.managerUrl
          : managerSettings.managerUrl;
      try {
        const token = await ensureManagerToken();
        if (!token) {
          setSessions([]);
          setError('Missing manager auth token');
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
        setSessions(data);
      } catch (e: any) {
        console.error('[dashboard] listSessions failed', {
          privateBeachId: id,
          managerUrl: effectiveManagerUrl,
          error: e,
        });
        setError(e.message || 'Failed to load sessions');
      } finally {
        setLoading(false);
      }
    },
    [id, managerSettings.managerUrl, ensureManagerToken],
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
        await refresh({ managerUrl: nextManager.managerUrl });
      } catch (err) {
        setSettingsJson(prevSettings);
        throw err;
      } finally {
        setSavingSettings(false);
      }
    },
    [id, settingsJson, managerSettings, defaultManagerUrl, refresh, ensureManagerToken],
  );

  useEffect(() => {
    refresh();
  }, [refresh]);

  const persistLayoutSnapshot = useCallback(
    (items: BeachLayout['layout']) => {
      if (!id) return;
      setLayout((prev) => {
        const allowed = new Set(prev.tiles);
        const filtered = items.filter((entry) => allowed.has(entry.id));
        const next = { ...prev, layout: filtered };
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
              <Button onClick={() => refresh()} disabled={loading}>{loading ? 'Loading…' : 'Refresh'}</Button>
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
              token={managerToken}
              managerUrl={managerUrl}
              refresh={() => refresh()}
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
            onAttached={() => refresh()}
          />
        )}
      </div>
    </BeachSettingsProvider>
  );
}
