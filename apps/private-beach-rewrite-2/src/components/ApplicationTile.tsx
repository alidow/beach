'use client';

import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import type { SessionSummary } from '@private-beach/shared-api';
import { attachByCode, fetchSessionStateSnapshot, updateSessionRoleById } from '@/lib/api';
import type { TileSessionMeta } from '@/features/tiles';
import type { SessionCredentialOverride } from '../../../private-beach/src/hooks/terminalViewerTypes';
import { useManagerToken, buildManagerUrl } from '../hooks/useManagerToken';
import { useSessionConnection } from '../hooks/useSessionConnection';
import { SessionViewer } from './SessionViewer';
import {
  hydrateTerminalStoreFromDiff,
  type CellStylePayload,
} from '../../../private-beach/src/lib/terminalHydrator';
import type { Update } from '../../../beach-surfer/src/protocol/types';

const DEFAULT_STYLE_ID = 0;

function sanitizeStyleId(raw: unknown, fallback = DEFAULT_STYLE_ID): number {
  if (typeof raw === 'number' && Number.isFinite(raw)) {
    const normalized = Math.trunc(raw);
    return normalized >= 0 ? normalized : fallback;
  }
  return fallback;
}

function buildStyleUpdates(styles: CellStylePayload[] | null | undefined, sequence: number): Update[] {
  const updates: Update[] = [];
  const seen = new Set<number>();
  if (Array.isArray(styles)) {
    for (const entry of styles) {
      if (!entry || typeof entry !== 'object') {
        continue;
      }
      const id = sanitizeStyleId(entry.id, DEFAULT_STYLE_ID);
      if (seen.has(id)) {
        continue;
      }
      seen.add(id);
      updates.push({
        type: 'style',
        id,
        seq: sequence,
        fg: typeof entry.fg === 'number' ? entry.fg : 0,
        bg: typeof entry.bg === 'number' ? entry.bg : 0,
        attrs: typeof entry.attrs === 'number' ? entry.attrs : 0,
      });
    }
  }
  if (!seen.has(DEFAULT_STYLE_ID)) {
    updates.push({
      type: 'style',
      id: DEFAULT_STYLE_ID,
      seq: sequence,
      fg: 0,
      bg: 0,
      attrs: 0,
    });
  }
  return updates;
}

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
  const prehydratedSequenceRef = useRef<string | null>(null);
  const cachedStyleUpdatesRef = useRef<Update[] | null>(null);
  const lastSessionIdRef = useRef<string | null>(sessionMeta?.sessionId ?? null);

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

  useEffect(() => {
    const currentSessionId = sessionMeta?.sessionId ?? null;
    if (lastSessionIdRef.current !== currentSessionId) {
      lastSessionIdRef.current = currentSessionId;
      prehydratedSequenceRef.current = null;
      cachedStyleUpdatesRef.current = null;
    }
  }, [sessionMeta?.sessionId]);

  useEffect(() => {
    const store = viewer.store;
    const sessionId = sessionMeta?.sessionId?.trim();
    if (!store || !sessionId || !managerUrl) {
      return;
    }
    let cancelled = false;
    const fetchAndHydrate = async () => {
      let token = managerToken?.trim();
      if (!token) {
        try {
          const refreshed = await refresh();
          token = refreshed?.trim() ?? '';
        } catch (refreshError) {
          if (typeof window !== 'undefined') {
            console.warn('[terminal][hydrate] token refresh failed', {
              sessionId,
              error: refreshError,
            });
          }
        }
      }
      if (!token || cancelled) {
        return;
      }
      try {
        const diff = await fetchSessionStateSnapshot(sessionId, token, managerUrl);
        if (!diff || cancelled) {
          return;
        }
        const sequenceKey = `${sessionId}:${diff.sequence ?? 0}`;
        if (prehydratedSequenceRef.current === sequenceKey) {
          return;
        }
        const hydrated = hydrateTerminalStoreFromDiff(store, diff, {});
        if (hydrated) {
          prehydratedSequenceRef.current = sequenceKey;
          cachedStyleUpdatesRef.current = buildStyleUpdates(diff.payload.styles ?? null, diff.sequence ?? 0);
          if (cachedStyleUpdatesRef.current.length > 0) {
            store.applyUpdates(cachedStyleUpdatesRef.current, {
              authoritative: false,
              origin: 'cached-style-refresh',
            });
          }
          if (typeof window !== 'undefined') {
            try {
              const snapshot = store.getSnapshot();
              console.info('[terminal][hydrate] applied cached diff', {
                sessionId,
                sequence: diff.sequence ?? 0,
                rows: snapshot.rows.length,
                baseRow: snapshot.baseRow,
                viewportTop: snapshot.viewportTop,
                viewportHeight: snapshot.viewportHeight,
              });
            } catch (error) {
              console.info('[terminal][hydrate] applied cached diff', {
                sessionId,
                sequence: diff.sequence ?? 0,
                snapshotError: error instanceof Error ? error.message : String(error),
              });
            }
          }
        }
      } catch (error) {
        if (typeof window !== 'undefined') {
          console.warn('[terminal][hydrate] snapshot fetch failed', { sessionId, error });
        }
      }
    };
    fetchAndHydrate();
    return () => {
      cancelled = true;
    };
  }, [managerToken, managerUrl, refresh, sessionMeta?.sessionId, viewer.store]);

  useEffect(() => {
    const store = viewer.store;
    const styleUpdates = cachedStyleUpdatesRef.current;
    if (!store || !styleUpdates || styleUpdates.length === 0) {
      return;
    }
    if (viewer.status !== 'connected' && viewer.status !== 'reconnecting') {
      return;
    }
    store.applyUpdates(styleUpdates, { authoritative: false, origin: 'cached-style-refresh' });
  }, [viewer.status, viewer.store]);

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

  const disabled = submitState !== 'idle' || tokenLoading;
  const hasSession = Boolean(sessionMeta?.sessionId);

  return (
    <div className="flex h-full min-h-0 flex-col gap-4 text-[13px] text-slate-200">
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
            disableViewportMeasurements={disableViewportMeasurements}
          />
        </div>
      )}
    </div>
  );
}
