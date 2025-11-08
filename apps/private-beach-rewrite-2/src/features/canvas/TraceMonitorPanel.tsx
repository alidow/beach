'use client';

import { useMemo } from 'react';
import type { ControllerUpdateCadence } from '@/lib/api';
import type { TraceLogEntry } from '@/features/trace/traceLogStore';

type PairingAttempt = {
  id: string;
  status: 'ok' | 'error';
  message: string | null;
  timestamp: number;
};

type TraceMonitorPanelProps = {
  traceId: string;
  agentRole?: string | null;
  agentResponsibility?: string | null;
  instructions?: string | null;
  cadence?: ControllerUpdateCadence | null;
  pollFrequency?: number | null;
  sourceSessionId?: string | null;
  targetSessionId?: string | null;
  pairingHistory: PairingAttempt[];
  logs: TraceLogEntry[];
  onClose: () => void;
};

export function TraceMonitorPanel({
  traceId,
  agentRole,
  agentResponsibility,
  instructions,
  cadence,
  pollFrequency,
  sourceSessionId,
  targetSessionId,
  pairingHistory,
  logs,
  onClose,
}: TraceMonitorPanelProps) {
  const latestStatus = pairingHistory[0];
  const logEntries = useMemo(() => logs.slice(-20).reverse(), [logs]);

  return (
    <div className="pointer-events-none absolute inset-0 z-40 flex justify-end bg-slate-950/40 backdrop-blur">
      <section className="pointer-events-auto flex h-full w-full max-w-xl flex-col overflow-hidden border-l border-white/10 bg-slate-950/95 text-slate-100 shadow-2xl">
        <header className="flex items-start justify-between border-b border-white/10 px-6 py-4">
          <div>
            <p className="text-[11px] font-semibold uppercase tracking-[0.3em] text-slate-400">Trace Monitor</p>
            <p className="mt-1 font-mono text-sm text-white">{traceId}</p>
            {latestStatus ? (
              <p className="mt-1 text-xs text-slate-300">
                Last sync:&nbsp;
                <span className={latestStatus.status === 'ok' ? 'text-emerald-300' : 'text-rose-300'}>
                  {latestStatus.status === 'ok' ? 'OK' : 'Error'}
                </span>
                &nbsp;at {new Date(latestStatus.timestamp).toLocaleTimeString()}
              </p>
            ) : (
              <p className="mt-1 text-xs text-slate-400">No sync attempts recorded for this trace.</p>
            )}
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded-full border border-white/15 px-3 py-1 text-xs font-semibold uppercase tracking-[0.2em] text-slate-200 transition hover:border-white/40 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60"
          >
            Close
          </button>
        </header>
        <div className="grid grid-cols-1 gap-6 overflow-y-auto px-6 py-4 sm:grid-cols-2">
          <div>
            <p className="text-[11px] font-semibold uppercase tracking-[0.3em] text-slate-400">Pairing</p>
            <dl className="mt-2 space-y-1 text-sm text-slate-200">
              <div>
                <dt className="text-xs uppercase tracking-widest text-slate-500">Controller</dt>
                <dd className="font-mono text-[11px] text-slate-200">{sourceSessionId ?? 'unknown'}</dd>
              </div>
              <div>
                <dt className="text-xs uppercase tracking-widest text-slate-500">Child</dt>
                <dd className="font-mono text-[11px] text-slate-200">{targetSessionId ?? 'unknown'}</dd>
              </div>
              <div>
                <dt className="text-xs uppercase tracking-widest text-slate-500">Cadence</dt>
                <dd className="text-slate-200">{cadence ?? 'balanced'}</dd>
              </div>
              <div>
                <dt className="text-xs uppercase tracking-widest text-slate-500">Poll Frequency</dt>
                <dd className="text-slate-200">{pollFrequency ? `${pollFrequency}s` : 'not configured'}</dd>
              </div>
            </dl>
          </div>
          <div>
            <p className="text-[11px] font-semibold uppercase tracking-[0.3em] text-slate-400">Prompt</p>
            <div className="mt-2 rounded-lg border border-white/10 bg-white/5 p-3 text-xs text-slate-100">
              <p className="font-semibold text-slate-200">{agentRole ?? 'Agent'}</p>
              <p className="mt-1 text-slate-300">{agentResponsibility ?? 'Responsibility not provided.'}</p>
              {instructions ? (
                <p className="mt-2 whitespace-pre-wrap text-[11px] text-slate-400">{instructions}</p>
              ) : null}
            </div>
          </div>
          <div className="sm:col-span-2">
            <p className="text-[11px] font-semibold uppercase tracking-[0.3em] text-slate-400">Pairing Sync Attempts</p>
            {pairingHistory.length === 0 ? (
              <p className="mt-2 text-xs text-slate-400">No batch assignment attempts recorded yet.</p>
            ) : (
              <ul className="mt-2 space-y-2">
                {pairingHistory.slice(0, 8).map((attempt) => (
                  <li
                    key={attempt.id}
                    className="rounded-lg border border-white/10 bg-white/5 px-3 py-2 text-xs text-slate-200"
                  >
                    <p>
                      <span
                        className={
                          attempt.status === 'ok'
                            ? 'font-semibold uppercase tracking-[0.3em] text-emerald-300'
                            : 'font-semibold uppercase tracking-[0.3em] text-rose-300'
                        }
                      >
                        {attempt.status === 'ok' ? 'Success' : 'Error'}
                      </span>
                      <span className="ml-2 text-slate-400">
                        {new Date(attempt.timestamp).toLocaleTimeString()}
                      </span>
                    </p>
                    {attempt.message ? <p className="mt-1 text-slate-300">{attempt.message}</p> : null}
                  </li>
                ))}
              </ul>
            )}
          </div>
          <div className="sm:col-span-2">
            <p className="text-[11px] font-semibold uppercase tracking-[0.3em] text-slate-400">Client Trace Logs</p>
            {logEntries.length === 0 ? (
              <p className="mt-2 text-xs text-slate-400">No trace logs captured yet.</p>
            ) : (
              <ul className="mt-2 space-y-2 text-xs text-slate-300">
                {logEntries.map((entry) => (
                  <li
                    key={entry.id}
                    className="rounded-lg border border-white/10 bg-slate-900/80 px-3 py-2 text-[11px] text-slate-200"
                  >
                    <p className="flex items-center justify-between">
                      <span className="font-semibold uppercase tracking-[0.3em] text-slate-400">{entry.source}</span>
                      <span className="text-slate-500">{new Date(entry.timestamp).toLocaleTimeString()}</span>
                    </p>
                    <p className="mt-1 text-slate-100">{entry.message}</p>
                    {entry.detail ? (
                      <pre className="mt-1 overflow-x-auto rounded bg-black/40 px-2 py-1 text-[10px] text-slate-400">
                        {JSON.stringify(entry.detail, null, 2)}
                      </pre>
                    ) : null}
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      </section>
    </div>
  );
}
