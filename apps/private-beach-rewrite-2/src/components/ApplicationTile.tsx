'use client';

import { useCallback, useEffect, useMemo, useState, type FormEvent } from 'react';
import type { SessionSummary } from '@private-beach/shared-api';
import { attachByCode, updateSessionRoleById } from '@/lib/api';
import type { TileSessionMeta } from '@/features/tiles';
import type { SessionCredentialOverride } from '../../../private-beach/src/hooks/terminalViewerTypes';
import { useManagerToken, buildManagerUrl } from '../hooks/useManagerToken';
import { useSessionConnection } from '../hooks/useSessionConnection';
import { SessionViewer } from './SessionViewer';
import { cn } from '@/lib/cn';

type ApplicationTileProps = {
  tileId: string;
  privateBeachId: string;
  managerUrl?: string;
  sessionMeta?: TileSessionMeta | null;
  onSessionMetaChange?: (meta: TileSessionMeta | null) => void;
  disableViewportMeasurements?: boolean;
};

type SubmitState = 'idle' | 'attaching';

function sessionSummaryToMeta(session: SessionSummary): TileSessionMeta {
  const metadata = session.metadata;
  let title: string | null = null;
  if (metadata && typeof metadata === 'object') {
    const record = metadata as Record<string, unknown>;
    if (typeof record.title === 'string') {
      title = record.title as string;
    } else if (typeof record.name === 'string') {
      title = record.name as string;
    }
  }
  return {
    sessionId: session.session_id,
    title: title ?? session.session_id,
    harnessType: session.harness_type ?? null,
    status: 'attached',
    pendingActions: session.pending_actions ?? 0,
  };
}

function statusLabel(status: string): string {
  switch (status) {
    case 'connected':
      return 'Connected';
    case 'reconnecting':
      return 'Reconnecting';
    case 'error':
      return 'Error';
    case 'connecting':
    default:
      return 'Connecting';
  }
}

export function ApplicationTile({
  tileId,
  privateBeachId,
  managerUrl = buildManagerUrl(),
  sessionMeta,
  onSessionMetaChange,
  disableViewportMeasurements = false,
}: ApplicationTileProps) {
  const [sessionIdInput, setSessionIdInput] = useState(sessionMeta?.sessionId ?? '');
  const [codeInput, setCodeInput] = useState('');
  const [submitState, setSubmitState] = useState<SubmitState>('idle');
  const [attachError, setAttachError] = useState<string | null>(null);
  const [roleWarning, setRoleWarning] = useState<string | null>(null);
  const [credentialOverride, setCredentialOverride] = useState<SessionCredentialOverride | null>(null);

  const {
    token: managerToken,
    loading: tokenLoading,
    error: tokenError,
    isLoaded,
    isSignedIn,
    refresh,
  } = useManagerToken();

  useEffect(() => {
    if (sessionMeta?.sessionId && sessionMeta.sessionId !== sessionIdInput) {
      setSessionIdInput(sessionMeta.sessionId);
    }
  }, [sessionMeta?.sessionId, sessionIdInput]);

  const viewer = useSessionConnection({
    tileId,
    sessionId: sessionMeta?.sessionId ?? null,
    privateBeachId,
    managerUrl,
    authToken: managerToken,
    credentialOverride: credentialOverride ?? undefined,
  });

  const connectionTone = useMemo(() => {
    switch (viewer.status) {
      case 'connected':
        return 'success';
      case 'reconnecting':
        return 'warning';
      case 'error':
        return 'danger';
      case 'connecting':
        return 'info';
      default:
        return 'muted';
    }
  }, [viewer.status]);

  const connectionLabel = useMemo(() => statusLabel(viewer.status), [viewer.status]);

  useEffect(() => {
    if (!sessionMeta || !onSessionMetaChange) {
      return;
    }
    const nextStatus = statusLabel(viewer.status);
    if (sessionMeta.status === nextStatus) {
      return;
    }
    onSessionMetaChange({ ...sessionMeta, status: nextStatus });
  }, [sessionMeta, viewer.status, onSessionMetaChange]);

  const handleAttach = useCallback(
    async (event: FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const trimmedSessionId = sessionIdInput.trim();
      const trimmedCode = codeInput.trim();

      if (!privateBeachId) {
        setAttachError('Missing private beach identifier.');
        return;
      }
      if (!trimmedSessionId) {
        setAttachError('Enter a session id before attaching.');
        return;
      }
      if (!trimmedCode) {
        setAttachError('Enter the 6-digit session code.');
        return;
      }

      setSubmitState('attaching');
      setAttachError(null);
      setRoleWarning(null);

      const token =
        managerToken && managerToken.trim().length > 0 ? managerToken : await refresh();
      if (!token) {
        setSubmitState('idle');
        setAttachError('Unable to fetch manager token. Sign in and try again.');
        return;
      }

      try {
        const response = await attachByCode(privateBeachId, trimmedSessionId, trimmedCode, token, managerUrl);
        const session = (response?.session ?? null) as SessionSummary | null;
        if (!session) {
          throw new Error('Attach response missing session payload.');
        }
        const nextMeta = sessionSummaryToMeta(session);
        onSessionMetaChange?.(nextMeta);
        setCredentialOverride({ passcode: trimmedCode });
        setCodeInput('');
        setSessionIdInput(session.session_id);
        try {
          await updateSessionRoleById(
            session.session_id,
            'application',
            token,
            managerUrl,
            session.metadata,
            session.location_hint ?? null,
          );
        } catch (roleErr) {
          const message = roleErr instanceof Error ? roleErr.message : String(roleErr);
          setRoleWarning(`Attached session, but updating role failed: ${message}`);
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setAttachError(message || 'Failed to attach session.');
      } finally {
        setSubmitState('idle');
      }
    },
    [codeInput, managerToken, managerUrl, onSessionMetaChange, privateBeachId, refresh, sessionIdInput],
  );

  const handleDisconnect = useCallback(() => {
    setCredentialOverride(null);
    setCodeInput('');
    onSessionMetaChange?.(null);
  }, [onSessionMetaChange]);

  const disabled = submitState !== 'idle' || tokenLoading;
  const hasSession = Boolean(sessionMeta?.sessionId);

  const toneClass = cn(
    'flex items-center justify-between gap-3 rounded-full border px-4 py-2 text-xs font-semibold tracking-[0.22em] uppercase',
    connectionTone === 'muted' && 'border-white/10 bg-white/5 text-slate-300',
    connectionTone === 'info' && 'border-sky-500/40 bg-sky-500/15 text-sky-100',
    connectionTone === 'success' && 'border-emerald-500/40 bg-emerald-500/15 text-emerald-100',
    connectionTone === 'warning' && 'border-amber-400/50 bg-amber-500/20 text-amber-100',
    connectionTone === 'danger' && 'border-red-500/50 bg-red-500/15 text-red-100',
  );

  return (
    <div className="flex h-full min-h-0 flex-col gap-4 text-[13px] text-slate-200">
      <div className={toneClass}>
        <span>{connectionLabel}</span>
        {viewer.latencyMs != null && viewer.latencyMs > 0 && (
          <span className="font-mono text-[11px] tracking-normal opacity-90">{Math.round(viewer.latencyMs)} ms</span>
        )}
      </div>

      {!hasSession ? (
        <form className="grid gap-3" onSubmit={handleAttach}>
          <label className="grid gap-1 text-[11px] font-semibold uppercase tracking-[0.18em] text-slate-400">
            <span>Session ID</span>
            <input
              value={sessionIdInput}
              onChange={(event) => setSessionIdInput(event.target.value)}
              placeholder="sess-1234…"
              autoComplete="off"
              className="h-10 rounded-full border border-white/10 bg-white/5 px-4 text-[13px] font-medium text-white placeholder:text-slate-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60"
            />
          </label>
          <label className="grid gap-1 text-[11px] font-semibold uppercase tracking-[0.18em] text-slate-400">
            <span>Passcode</span>
            <input
              value={codeInput}
              onChange={(event) => setCodeInput(event.target.value)}
              placeholder="6-digit code"
              autoComplete="off"
              className="h-10 rounded-full border border-white/10 bg-white/5 px-4 text-[13px] font-medium text-white placeholder:text-slate-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60"
            />
          </label>
          <button
            type="submit"
            disabled={disabled}
            className="mt-2 inline-flex h-10 items-center justify-center rounded-full border border-sky-400/60 bg-sky-500/20 px-6 text-sm font-semibold uppercase tracking-[0.18em] text-sky-100 transition hover:border-sky-300/80 hover:bg-sky-500/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {submitState === 'attaching' ? 'Attaching…' : 'Connect'}
          </button>
          {!isLoaded && <p className="text-[11px] text-slate-400">Loading authentication…</p>}
          {isLoaded && !isSignedIn && (
            <p className="text-[11px] text-slate-400">Sign in with Clerk to request manager credentials.</p>
          )}
          {tokenError && (
            <p className="rounded-xl border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-100">{tokenError}</p>
          )}
          {attachError && (
            <p className="rounded-xl border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-100">{attachError}</p>
          )}
        </form>
      ) : (
        <div className="flex flex-1 min-h-0 flex-col gap-4">
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-2xl border border-white/10 bg-white/5 px-4 py-3">
            <div className="min-w-0">
              <strong
                className="block truncate text-sm font-semibold text-white"
                title={sessionMeta?.title ?? sessionMeta?.sessionId ?? undefined}
              >
                {sessionMeta?.title ?? sessionMeta?.sessionId ?? 'Attached Session'}
              </strong>
              <span className="text-[11px] uppercase tracking-[0.18em] text-slate-400">
                {sessionMeta?.harnessType ?? 'Unknown harness'}
              </span>
            </div>
            <button
              type="button"
              onClick={handleDisconnect}
              className="inline-flex h-8 items-center justify-center rounded-full border border-white/20 px-4 text-[11px] font-semibold uppercase tracking-[0.22em] text-slate-200 transition hover:border-white/40 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60"
            >
              Disconnect
            </button>
          </div>
          {roleWarning && (
            <p className="rounded-xl border border-amber-400/40 bg-amber-400/10 px-3 py-2 text-xs text-amber-100">
              {roleWarning}
            </p>
          )}
          {attachError && (
            <p className="rounded-xl border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-100">{attachError}</p>
          )}
          {viewer.status === 'error' && viewer.error && (
            <p className="rounded-xl border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-100">
              {viewer.error}
            </p>
          )}
          <SessionViewer
            viewer={viewer}
            sessionId={sessionMeta?.sessionId ?? null}
            className="relative flex min-h-[220px] flex-1 overflow-hidden rounded-2xl border border-white/10 bg-slate-950/80 shadow-[0_30px_80px_rgba(2,6,23,0.65)] backdrop-blur-xl"
            disableViewportMeasurements={disableViewportMeasurements}
          />
        </div>
      )}
    </div>
  );
}
