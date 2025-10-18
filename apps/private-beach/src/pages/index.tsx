import { useEffect, useMemo, useRef, useState } from 'react';
import SessionTable from '../components/SessionTable';
import { SessionSummary, listSessions, acquireController, releaseController, eventsSseUrl, stateSseUrl, emergencyStop } from '../lib/api';

function useLocalStorage(key: string, initial: string) {
  const [value, setValue] = useState<string>(() => (typeof window !== 'undefined' ? localStorage.getItem(key) || initial : initial));
  useEffect(() => {
    if (typeof window !== 'undefined') localStorage.setItem(key, value);
  }, [key, value]);
  return [value, setValue] as const;
}

export default function Dashboard() {
  const [managerUrl, setManagerUrl] = useLocalStorage('pb.manager', process.env.NEXT_PUBLIC_MANAGER_URL || 'http://localhost:3000');
  const [beachId, setBeachId] = useLocalStorage('pb.beach', '');
  const [token, setToken] = useLocalStorage('pb.token', 'test-token');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [selected, setSelected] = useState<SessionSummary | null>(null);
  const [events, setEvents] = useState<string[]>([]);
  const [countdown, setCountdown] = useState<string>('');
  const sseRef = useRef<EventSource | null>(null);
  const sseStateRef = useRef<EventSource | null>(null);

  useEffect(() => {
    // Keep NEXT_PUBLIC_MANAGER_URL in sync if user edits the input
    if (typeof window !== 'undefined') {
      (window as any).NEXT_PUBLIC_MANAGER_URL = managerUrl;
    }
  }, [managerUrl]);

  async function refresh() {
    if (!beachId) return;
    setLoading(true);
    setError(null);
    try {
      const data = await listSessions(beachId, token, managerUrl);
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
  }, [beachId, token, managerUrl]);

  useEffect(() => {
    // bind SSE for selected session
    sseRef.current?.close();
    sseStateRef.current?.close();
    setEvents([]);
    setCountdown('');
    if (!selected) return;
    const ev = new EventSource(eventsSseUrl(selected.session_id, managerUrl, token));
    ev.addEventListener('controller_event', (msg: MessageEvent) => {
      setEvents((prev) => [msg.data, ...prev].slice(0, 200));
    });
    const st = new EventSource(stateSseUrl(selected.session_id, managerUrl, token));
    st.addEventListener('state', (msg: MessageEvent) => {
      setEvents((prev) => [msg.data, ...prev].slice(0, 200));
    });
    sseRef.current = ev;
    sseStateRef.current = st;
    // lease countdown
    let timer: any;
    const updateCountdown = () => {
      const t = selected.controller_expires_at_ms;
      if (!t) { setCountdown(''); return; }
      const remain = Math.max(0, t - Date.now());
      const s = Math.floor(remain / 1000);
      setCountdown(`${s}s`);
    };
    updateCountdown();
    timer = setInterval(updateCountdown, 1000);
    return () => {
      ev.close();
      st.close();
      if (timer) clearInterval(timer);
    };
  }, [selected, managerUrl, token]);

  async function onAcquire() {
    if (!selected) return;
    try {
      const resp = await acquireController(selected.session_id, 30000, token, managerUrl);
      setEvents((prev) => [`lease acquired: ${JSON.stringify(resp)}`, ...prev]);
      refresh();
    } catch (e: any) {
      setEvents((prev) => [`acquire failed: ${e.message}`, ...prev]);
    }
  }

  async function onRelease() {
    if (!selected) return;
    const controllerToken = selected.controller_token;
    if (!controllerToken) {
      setEvents((prev) => ['no controller token on selected session', ...prev]);
      return;
    }
    try {
      await releaseController(selected.session_id, controllerToken, token, managerUrl);
      setEvents((prev) => ['lease released', ...prev]);
      refresh();
    } catch (e: any) {
      setEvents((prev) => [`release failed: ${e.message}`, ...prev]);
    }
  }

  async function onStop() {
    if (!selected) return;
    try {
      await emergencyStop(selected.session_id, token, managerUrl);
      setEvents((prev) => ['emergency stop issued', ...prev]);
      refresh();
    } catch (e: any) {
      setEvents((prev) => [`stop failed: ${e.message}`, ...prev]);
    }
  }

  return (
    <div style={{ maxWidth: 1100, margin: '0 auto', padding: 24 }}>
      <h1 style={{ marginBottom: 12 }}>Private Beach Surfer</h1>
      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 12, marginBottom: 12 }}>
        <label style={labelStyle}>
          Manager URL
          <input style={inputStyle} value={managerUrl} onChange={(e) => setManagerUrl(e.target.value)} placeholder="http://localhost:3000" />
        </label>
        <label style={labelStyle}>
          Private Beach ID
          <input style={inputStyle} value={beachId} onChange={(e) => setBeachId(e.target.value)} placeholder="<uuid>" />
        </label>
        <label style={labelStyle}>
          Token
          <input style={inputStyle} value={token} onChange={(e) => setToken(e.target.value)} placeholder="test-token" />
        </label>
      </div>
      <div style={{ marginBottom: 8 }}>
        <button onClick={refresh} disabled={loading || !beachId} style={btn}>Refresh</button>
      </div>
      {error && <div style={err}>{error}</div>}
      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 16 }}>
        <div>
          <h3>Sessions</h3>
          <SessionTable sessions={sessions} onSelect={setSelected} />
        </div>
        <div>
          <h3>Selected</h3>
          {!selected ? (
            <div style={{ color: '#666' }}>Click a session to view details</div>
          ) : (
            <div>
              <div style={{ marginBottom: 8 }}>
                <div style={{ fontFamily: 'ui-monospace, monospace' }}>{selected.session_id}</div>
                <div style={{ fontSize: 12, color: '#666' }}>harness: {selected.harness_type}</div>
              </div>
              {selected.controller_token && (
                <div style={{ fontSize: 12, color: '#333', marginBottom: 8 }}>
                  lease expires in: {countdown || '—'}
                </div>
              )}
              <div style={{ display: 'flex', gap: 8, marginBottom: 8 }}>
                <button onClick={onAcquire} style={btn}>Acquire Lease</button>
                <button onClick={onRelease} style={btn}>Release Lease</button>
                <button onClick={onStop} style={{ ...btn, borderColor: '#f66', color: '#900' }}>Emergency Stop</button>
              </div>
              <div style={{ background: '#fff', border: '1px solid #eee', height: 280, overflow: 'auto', padding: 8 }}>
                {events.length === 0 ? (
                  <div style={{ color: '#999' }}>No events yet…</div>
                ) : (
                  events.map((e, i) => (
                    <pre key={i} style={{ margin: 0, fontSize: 12, borderBottom: '1px dashed #f0f0f0', padding: '4px 0' }}>{e}</pre>
                  ))
                )}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

const labelStyle: React.CSSProperties = { display: 'flex', flexDirection: 'column', gap: 4, fontSize: 12, color: '#333' };
const inputStyle: React.CSSProperties = { padding: 8, border: '1px solid #ddd', borderRadius: 6 };
const btn: React.CSSProperties = { padding: '8px 12px', border: '1px solid #ddd', borderRadius: 6, background: '#fff', cursor: 'pointer' };
const err: React.CSSProperties = { marginBottom: 8, padding: 8, background: '#fee', color: '#900', border: '1px solid #f99', borderRadius: 6 };
