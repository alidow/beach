import { useEffect, useMemo, useState } from 'react';
import { attachByCode, attachOwned } from '../lib/api';
import { listMySessions, RoadMySession } from '../lib/road';
import { Dialog } from './ui/dialog';
import { Input } from './ui/input';
import { Button } from './ui/button';

type Props = {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  privateBeachId: string;
  managerUrl: string;
  roadUrl?: string;
  token: string | null;
  onAttached?: (ids: string[]) => void;
};

export default function AddSessionModal({ open, onOpenChange, privateBeachId, managerUrl, roadUrl, token, onAttached }: Props) {
  const [tab, setTab] = useState<'code' | 'mine' | 'new'>('code');
  const [sessionId, setSessionId] = useState('');
  const [code, setCode] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [mine, setMine] = useState<RoadMySession[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const hasToken = token && token.trim().length > 0;

  useEffect(() => {
    if (tab !== 'mine') {
      setError(null);
    }
  }, [tab]);

  useEffect(() => {
    if (!open) return;
    if (tab === 'mine') {
      if (!hasToken) {
        setMine([]);
        setError('Sign in to load your active sessions.');
        return;
      }
      listMySessions(token, roadUrl)
        .then((sessions) => {
          setMine(sessions);
          setError(null);
        })
        .catch((e) => setError(e.message || 'Failed to load sessions'));
    }
  }, [open, tab, token, roadUrl, hasToken]);

  async function submitCode() {
    if (!hasToken) {
      setError('Sign in to attach sessions.');
      return;
    }
    setLoading(true); setError(null);
    try {
      const resp = await attachByCode(privateBeachId, sessionId.trim(), code.trim(), token, managerUrl);
      onAttached?.([resp.session.session_id]);
      onOpenChange(false);
    } catch (e: any) {
      setError(e.message || 'Attach failed');
    } finally { setLoading(false); }
  }

  async function submitMine() {
    if (selected.size === 0) return;
    if (!hasToken) {
      setError('Sign in to attach sessions.');
      return;
    }
    setLoading(true); setError(null);
    try {
      const ids = Array.from(selected);
      await attachOwned(privateBeachId, ids, token, managerUrl);
      onAttached?.(ids);
      onOpenChange(false);
    } catch (e: any) {
      setError(e.message || 'Attach failed');
    } finally { setLoading(false); }
  }

  const command = `beach run --private-beach ${privateBeachId} --title "My Session"`;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <div className="w-[560px] max-w-[96vw] rounded-lg border border-border bg-card text-card-foreground shadow">
        <div className="border-b border-border p-3 text-sm font-semibold">Add Session</div>
        <div className="flex items-center gap-3 border-b border-border px-3 text-sm">
          <button className={`py-2 transition-colors ${tab === 'code' ? 'font-semibold text-foreground' : 'text-muted-foreground hover:text-foreground'}`} onClick={() => setTab('code')}>By Code</button>
          <button className={`py-2 transition-colors ${tab === 'mine' ? 'font-semibold text-foreground' : 'text-muted-foreground hover:text-foreground'}`} onClick={() => setTab('mine')}>My Sessions</button>
          <button className={`py-2 transition-colors ${tab === 'new' ? 'font-semibold text-foreground' : 'text-muted-foreground hover:text-foreground'}`} onClick={() => setTab('new')}>Launch New</button>
        </div>
        <div className="p-3">
          {tab==='code' && (
            <div className="space-y-2">
              <div className="text-sm text-muted-foreground">Attach a public Beach session by its ID + code.</div>
              <Input placeholder="Session ID (UUID)" value={sessionId} onChange={(e) => setSessionId(e.target.value)} />
              <Input placeholder="6-digit code" value={code} onChange={(e) => setCode(e.target.value)} />
              {error && <div className="rounded border border-red-200/80 bg-red-500/10 p-2 text-xs text-red-600 dark:text-red-400">{error}</div>}
              <div className="flex justify-end">
                <Button onClick={submitCode} disabled={loading || !sessionId || !code || !hasToken}>{loading ? 'Verifying…' : 'Attach'}</Button>
              </div>
            </div>
          )}
          {tab==='mine' && (
            <div className="space-y-2">
              <div className="text-sm text-muted-foreground">Pick from your active sessions.</div>
              <div className="max-h-64 overflow-auto rounded border border-border">
                {mine.length === 0 ? (
                  <div className="p-2 text-sm text-muted-foreground">No active sessions.</div>
                ) : (
                  <ul>
                    {mine.map((s) => (
                      <li key={s.origin_session_id} className="flex items-center justify-between border-b border-border/70 p-2 last:border-b-0">
                        <div>
                          <div className="font-mono text-[11px]">{s.origin_session_id.slice(0,8)}</div>
                          <div className="text-[11px] text-muted-foreground">{s.kind} · {s.location_hint || '—'}</div>
                        </div>
                        <label className="text-xs text-muted-foreground">
                          <input type="checkbox" className="mr-2 accent-primary" checked={selected.has(s.origin_session_id)} onChange={(e) => {
                            const next = new Set(selected);
                            if (e.target.checked) next.add(s.origin_session_id); else next.delete(s.origin_session_id);
                            setSelected(next);
                          }} /> Select
                        </label>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
              {error && <div className="rounded border border-red-200/80 bg-red-500/10 p-2 text-xs text-red-600 dark:text-red-400">{error}</div>}
              <div className="flex justify-end">
                <Button onClick={submitMine} disabled={loading || selected.size===0 || !hasToken}>{loading ? 'Attaching…' : `Attach ${selected.size} session(s)`}</Button>
              </div>
            </div>
          )}
          {tab==='new' && (
            <div className="space-y-2">
              <div className="text-sm text-muted-foreground">Start a new CLI session already bound to this beach.</div>
              <pre className="overflow-auto rounded border border-border bg-muted p-2 text-[11px] text-muted-foreground">{command}</pre>
              <div className="text-[11px] text-muted-foreground">Requires beach CLI login.</div>
            </div>
          )}
        </div>
      </div>
    </Dialog>
  );
}
