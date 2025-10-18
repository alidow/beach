import { useEffect, useMemo, useState } from 'react';
import { SessionSummary, acquireController, emergencyStop, releaseController } from '../lib/api';
import { Badge } from './ui/badge';
import { Button } from './ui/button';

export type Tile = {
  session: SessionSummary;
};

type Props = {
  tiles: SessionSummary[];
  onRemove: (sessionId: string) => void;
  onSelect: (s: SessionSummary) => void;
  token: string | null;
  managerUrl: string;
  refresh: () => void;
  preset?: 'grid2x2' | 'onePlusThree' | 'focus';
};

export default function TileCanvas({ tiles, onRemove, onSelect, token, managerUrl, refresh, preset = 'grid2x2' }: Props) {
  const [now, setNow] = useState<number>(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);

  const gridClass = useMemo(() => {
    if (preset === 'focus') return 'grid-cols-1';
    if (preset === 'onePlusThree') return 'grid-cols-2 md:grid-cols-3';
    // grid2x2 default
    const count = tiles.length;
    if (count <= 1) return 'grid-cols-1';
    if (count <= 2) return 'grid-cols-2';
    if (count <= 4) return 'grid-cols-2 md:grid-cols-2';
    return 'grid-cols-3';
  }, [tiles.length, preset]);

  return (
    <div className={`grid ${gridClass} gap-3`}>
      {tiles.map((s) => {
        const expires = s.controller_expires_at_ms || 0;
        const remain = Math.max(0, expires - now);
        const countdown = s.controller_token ? `${Math.floor(remain / 1000)}s` : '';
        return (
          <div key={s.session_id} className="relative overflow-hidden rounded-lg border border-neutral-200 bg-white shadow-sm">
            <div className="absolute right-2 top-2 z-10 flex items-center gap-2">
              <Badge variant={s.last_health?.degraded ? 'warning' : 'success'}>{s.last_health?.degraded ? 'degraded' : 'ok'}</Badge>
              <Badge variant="muted">{s.pending_actions}/{s.pending_unacked}</Badge>
              {s.controller_token && <Badge variant="muted">{countdown}</Badge>}
            </div>
            <div className="absolute left-2 top-2 z-10 flex items-center gap-2">
              <span className="font-mono text-xs bg-white/60 px-1 rounded">{s.session_id.slice(0, 8)}</span>
              <span className="text-[11px] text-neutral-700 bg-white/60 px-1 rounded">{s.harness_type}</span>
            </div>
            <div className="flex h-48 items-center justify-center bg-neutral-50">
              <div className="text-sm text-neutral-500">Live stream placeholder</div>
            </div>
            <div className="border-t border-neutral-200 p-2">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <Button size="sm" variant="outline" onClick={() => onSelect(s)}>Details</Button>
                  <Button size="sm" onClick={async () => { await acquireController(s.session_id, 30000, token, managerUrl).catch(()=>{}); refresh(); }}>Acquire</Button>
                  <Button size="sm" variant="outline" onClick={async () => { if (!s.controller_token) return; await releaseController(s.session_id, s.controller_token, token, managerUrl).catch(()=>{}); refresh(); }}>Release</Button>
                  <Button size="sm" variant="danger" onClick={async () => { if (!confirm('Emergency stop?')) return; await emergencyStop(s.session_id, token, managerUrl).catch(()=>{}); refresh(); }}>Stop</Button>
                </div>
                <button className="text-xs text-neutral-600 underline" onClick={() => onRemove(s.session_id)}>Remove</button>
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}
