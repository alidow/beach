import { useRouter } from 'next/router';
import { useCallback, useEffect, useMemo, useState } from 'react';
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
  const [layout, setLayout] = useState<BeachLayout>({ tiles: [], preset: 'grid2x2' });
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
      token: typeof raw?.token === 'string' ? raw.token : '',
    };
  }, [settingsJson, defaultManagerUrl, defaultRoadUrl]);

  const managerUrl = managerSettings.managerUrl;
  const roadUrl = managerSettings.roadUrl;
  const token = managerSettings.token ? managerSettings.token : null;

  useEffect(() => {
    if (!id) return;
    (async () => {
      try {
        const meta = await getBeachMeta(id, token);
        setBeachName(meta.name);
        setSettingsJson(meta.settings || {});
      } catch (e: any) {
        if (e.message === 'not_found') setError('Beach not found'); else setError('Failed to load beach');
      }
      try {
        const l = await getBeachLayout(id, token);
        setLayout(l);
      } catch {
        // ignore, keep default
      }
    })();
  }, [id, token]);

  type RefreshOverride = { managerUrl?: string; token?: string | null };

  const refresh = useCallback(
    async (override?: RefreshOverride) => {
      if (!id) return;
      setLoading(true);
      setError(null);
      const effectiveManagerUrl = override?.managerUrl && override.managerUrl.length > 0 ? override.managerUrl : managerSettings.managerUrl;
      const overrideToken = override?.token;
      const effectiveToken =
        overrideToken !== undefined
          ? overrideToken && overrideToken.length > 0
            ? overrideToken
            : null
          : managerSettings.token && managerSettings.token.length > 0
            ? managerSettings.token
            : null;
      try {
        const data = await listSessions(id, effectiveToken, effectiveManagerUrl);
        setSessions(data);
      } catch (e: any) {
        setError(e.message || 'Failed to load sessions');
      } finally {
        setLoading(false);
      }
    },
    [id, managerSettings],
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
        token: partial.token !== undefined ? partial.token : managerSettings.token,
      };
      const nextSettings = {
        ...(prevSettings && typeof prevSettings === 'object' ? prevSettings : {}),
        manager: {
          manager_url: nextManager.managerUrl,
          road_url: nextManager.roadUrl,
          token: nextManager.token,
        },
      };
      setSettingsJson(nextSettings);
      setSavingSettings(true);
      try {
        await updateBeach(
          id,
          { settings: nextSettings },
          nextManager.token && nextManager.token.length > 0 ? nextManager.token : null,
          nextManager.managerUrl && nextManager.managerUrl.length > 0 ? nextManager.managerUrl : defaultManagerUrl,
        );
        await refresh({ managerUrl: nextManager.managerUrl, token: nextManager.token });
      } catch (err) {
        setSettingsJson(prevSettings);
        throw err;
      } finally {
        setSavingSettings(false);
      }
    },
    [id, settingsJson, managerSettings, defaultManagerUrl, refresh],
  );

  useEffect(() => {
    refresh();
  }, [refresh]);

  function addTile(sessionId: string) {
    if (!id) return;
    const next = { ...layout, tiles: Array.from(new Set([sessionId, ...layout.tiles])).slice(0, 6) };
    setLayout(next);
    putBeachLayout(id, next, token).catch(() => {});
  }
  function removeTile(sessionId: string) {
    if (!id) return;
    const next = { ...layout, tiles: layout.tiles.filter((t) => t !== sessionId) };
    setLayout(next);
    putBeachLayout(id, next, token).catch(() => {});
  }
  function changePreset(preset: BeachLayout['preset']) {
    if (!id) return;
    const next = { ...layout, preset };
    setLayout(next);
    putBeachLayout(id, next, token).catch(() => {});
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
            <div className="h-[calc(100vh-4rem)] rounded-lg border border-neutral-200 bg-white">
              <SessionListPanel sessions={sessions} onAdd={addTile} />
            </div>
          </div>
          <div className="col-span-12 md:col-span-9">
            {error && <div className="mb-2 rounded-md border border-red-200 bg-red-50 p-2 text-sm text-red-700">{error}</div>}
            <TileCanvas
              tiles={tileSessions}
              onRemove={removeTile}
              onSelect={onSelect}
              token={token}
              managerUrl={managerUrl}
              refresh={() => refresh()}
              preset={layout.preset}
            />
          </div>
        </div>
        <SessionDrawer open={drawerOpen} onOpenChange={setDrawerOpen} session={selected} managerUrl={managerUrl} token={token} />
        {id && (
          <AddSessionModal
            open={addOpen}
            onOpenChange={setAddOpen}
            privateBeachId={id}
            managerUrl={managerUrl}
            roadUrl={roadUrl}
            token={token}
            onAttached={() => refresh()}
          />
        )}
      </div>
    </BeachSettingsProvider>
  );
}
