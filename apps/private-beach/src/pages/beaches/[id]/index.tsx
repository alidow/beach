import { useRouter } from 'next/router';
import { useEffect, useMemo, useState } from 'react';
import TopNav from '../../../components/TopNav';
import SessionListPanel from '../../../components/SessionListPanel';
import TileCanvas from '../../../components/TileCanvas';
import SessionDrawer from '../../../components/SessionDrawer';
import { Button } from '../../../components/ui/button';
import { Select } from '../../../components/ui/select';
import { SessionSummary, listSessions } from '../../../lib/api';
import { BeachLayout, PrivateBeach, getBeach, loadLayout, saveLayout } from '../../../lib/beaches';

export default function BeachDashboard() {
  const router = useRouter();
  const { id } = router.query as { id?: string };
  const [beach, setBeach] = useState<PrivateBeach | null>(null);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [layout, setLayout] = useState<BeachLayout>({ tiles: [], preset: 'grid2x2' });
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [selected, setSelected] = useState<SessionSummary | null>(null);

  useEffect(() => {
    if (!id) return;
    const b = getBeach(id);
    setBeach(b);
    setLayout(loadLayout(id));
  }, [id]);

  const managerUrl = beach?.managerUrl || process.env.NEXT_PUBLIC_MANAGER_URL || 'http://localhost:8080';
  const token = beach?.token || null;

  async function refresh() {
    if (!beach) return;
    setLoading(true);
    setError(null);
    try {
      const data = await listSessions(beach.id, token, managerUrl);
      setSessions(data);
    } catch (e: any) {
      setError(e.message || 'Failed to load sessions');
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [beach?.id, managerUrl, token]);

  function addTile(sessionId: string) {
    if (!id) return;
    const next = { ...layout, tiles: Array.from(new Set([sessionId, ...layout.tiles])).slice(0, 6) };
    setLayout(next);
    saveLayout(id, next);
  }
  function removeTile(sessionId: string) {
    if (!id) return;
    const next = { ...layout, tiles: layout.tiles.filter((t) => t !== sessionId) };
    setLayout(next);
    saveLayout(id, next);
  }
  function changePreset(preset: BeachLayout['preset']) {
    if (!id) return;
    const next = { ...layout, preset };
    setLayout(next);
    saveLayout(id, next);
  }

  const tileSessions = useMemo(() => {
    const byId = new Map(sessions.map((s) => [s.session_id, s] as const));
    return layout.tiles.map((id) => byId.get(id)).filter(Boolean) as SessionSummary[];
  }, [sessions, layout.tiles]);

  function onSelect(s: SessionSummary) {
    setSelected(s);
    setDrawerOpen(true);
  }

  return (
    <div className="min-h-screen">
      <TopNav current={beach} onSwitch={(v) => router.push(`/beaches/${v}`)} right={
        <div className="flex items-center gap-2">
          <Select value={layout.preset} onChange={(v) => changePreset(v as any)} options={[
            { value: 'grid2x2', label: 'Layout: Grid' },
            { value: 'onePlusThree', label: 'Layout: 1+3' },
            { value: 'focus', label: 'Layout: Focus' },
          ]} />
          <Button onClick={refresh} disabled={loading}>{loading ? 'Loading…' : 'Refresh'}</Button>
        </div>
      } />
      <div className="grid grid-cols-12 gap-3 p-3">
        <div className="col-span-12 md:col-span-3">
          <div className="h-[calc(100vh-4rem)] rounded-lg border border-neutral-200 bg-white">
            <SessionListPanel sessions={sessions} onAdd={addTile} />
          </div>
        </div>
        <div className="col-span-12 md:col-span-9">
          {error && <div className="mb-2 rounded-md border border-red-200 bg-red-50 p-2 text-sm text-red-700">{error}</div>}
          <TileCanvas tiles={tileSessions} onRemove={removeTile} onSelect={onSelect} token={token} managerUrl={managerUrl} refresh={refresh} preset={layout.preset} />
        </div>
      </div>
      <SessionDrawer open={drawerOpen} onOpenChange={setDrawerOpen} session={selected} managerUrl={managerUrl} token={token} />
    </div>
  );
}

