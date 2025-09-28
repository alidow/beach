import { useState } from 'react';
import { BeachTerminal } from './components/BeachTerminal';

export default function App(): JSX.Element {
  const [sessionId, setSessionId] = useState('');
  const [baseUrl, setBaseUrl] = useState('http://127.0.0.1:8080');
  const [passcode, setPasscode] = useState('');
  const [autoConnect, setAutoConnect] = useState(false);

  return (
    <main className="min-h-screen bg-slate-950 text-slate-100 flex items-center justify-center p-6">
      <section className="w-full max-w-3xl rounded-2xl bg-slate-900 shadow-2xl shadow-slate-950/40 flex flex-col gap-6 p-10">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Beach Web</h1>
          <p className="text-slate-300">Experimental React/WebRTC terminal client.</p>
        </div>

        <div className="grid gap-4">
          <label className="flex flex-col gap-2 text-sm font-semibold uppercase tracking-[0.08em] text-slate-300">
            Session ID
            <input
              value={sessionId}
              onChange={(event) => setSessionId(event.target.value)}
              placeholder="00000000-0000-0000-0000-000000000000"
              className="rounded-lg border border-slate-700/60 bg-slate-950/60 px-4 py-3 text-base text-slate-100 placeholder:text-slate-500 focus:border-sky-500 focus:outline-none focus:ring-2 focus:ring-sky-500/40"
            />
          </label>
          <label className="flex flex-col gap-2 text-sm font-semibold uppercase tracking-[0.08em] text-slate-300">
            Base URL
            <input
              value={baseUrl}
              onChange={(event) => setBaseUrl(event.target.value)}
              placeholder="http://127.0.0.1:8080"
              className="rounded-lg border border-slate-700/60 bg-slate-950/60 px-4 py-3 text-base text-slate-100 placeholder:text-slate-500 focus:border-sky-500 focus:outline-none focus:ring-2 focus:ring-sky-500/40"
            />
          </label>
          <label className="flex flex-col gap-2 text-sm font-semibold uppercase tracking-[0.08em] text-slate-300">
            Passcode
            <input
              value={passcode}
              onChange={(event) => setPasscode(event.target.value)}
              placeholder="optional"
              className="rounded-lg border border-slate-700/60 bg-slate-950/60 px-4 py-3 text-base text-slate-100 placeholder:text-slate-500 focus:border-sky-500 focus:outline-none focus:ring-2 focus:ring-sky-500/40"
            />
          </label>
        </div>

        <label className="flex items-center gap-3 text-sm font-semibold uppercase tracking-[0.08em] text-slate-300">
          <input
            type="checkbox"
            checked={autoConnect}
            onChange={(event) => setAutoConnect(event.target.checked)}
            className="size-4 rounded border border-slate-600 bg-slate-950/80 text-sky-500 focus:outline-none focus:ring-2 focus:ring-sky-500/40"
          />
          Auto connect
        </label>

        <div className="rounded-xl border border-slate-700/50 bg-slate-950/90 p-3 h-[60vh] min-h-[40vh] flex flex-col">
          <BeachTerminal
            sessionId={sessionId || undefined}
            baseUrl={baseUrl || undefined}
            passcode={passcode || undefined}
            autoConnect={autoConnect}
            className="flex-1 min-h-0"
          />
        </div>
      </section>
    </main>
  );
}
