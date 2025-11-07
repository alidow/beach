'use client';

import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import type { SessionSummary } from '@private-beach/shared-api';
import { attachByCode, fetchSessionStateSnapshot, updateSessionRoleById } from '@/lib/api';
import type { TileSessionMeta } from '@/features/tiles';
import { buildSessionMetadataWithTile, sessionSummaryToTileMeta } from '@/features/tiles/sessionMeta';
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

function logHydration(event: string, detail: Record<string, unknown>) {
  if (typeof window === 'undefined') {
    return;
  }
  try {
    console.info('[terminal][hydrate]', event, JSON.stringify(detail ?? {}));
  } catch (error) {
    console.info('[terminal][hydrate]', event, {
      fallback: true,
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

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
  const hydrationRetryTimerRef = useRef<number | null>(null);
  const hydrationAttemptRef = useRef(0);

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
      hydrationAttemptRef.current = 0;
      if (hydrationRetryTimerRef.current != null && typeof window !== 'undefined') {
        window.clearTimeout(hydrationRetryTimerRef.current);
      }
      hydrationRetryTimerRef.current = null;
    }
  }, [sessionMeta?.sessionId]);

  useEffect(() => {
    const store = viewer.store;
    const sessionId = sessionMeta?.sessionId?.trim();
    if (!store || !sessionId || !managerUrl) {
      logHydration('skip:missing-context', {
        hasStore: Boolean(store),
        sessionIdPresent: Boolean(sessionId),
        hasManagerUrl: Boolean(managerUrl),
      });
      return;
    }
    let cancelled = false;
    const scheduleRetry = (reason: string) => {
      if (cancelled || typeof window === 'undefined') {
        return;
      }
      if (hydrationRetryTimerRef.current != null) {
        return;
      }
      const backoffStep = Math.min(hydrationAttemptRef.current, 5);
      const delay = Math.min(1000 * 2 ** backoffStep, 10000);
      hydrationRetryTimerRef.current = window.setTimeout(() => {
        hydrationRetryTimerRef.current = null;
        void fetchAndHydrate();
      }, delay);
      logHydration('retry-scheduled', {
        sessionId,
        reason,
        delayMs: delay,
        attempt: hydrationAttemptRef.current,
      });
    };

    const fetchAndHydrate = async () => {
      hydrationAttemptRef.current += 1;
      let token = managerToken?.trim();
      if (!token) {
        try {
          const refreshed = await refresh();
          token = refreshed?.trim() ?? '';
        } catch (refreshError) {
          logHydration('token-refresh-failed', {
            sessionId,
            error: refreshError instanceof Error ? refreshError.message : String(refreshError),
          });
        }
      }
      if (!token || cancelled) {
        logHydration('skip:no-token', {
          sessionId,
          hadInitialToken: Boolean(managerToken?.trim()),
          cancelled,
        });
        scheduleRetry('no-token');
        return;
      }
      try {
        logHydration('fetch:start', {
          sessionId,
          hasToken: Boolean(token),
          cancelled,
        });
        const diff = await fetchSessionStateSnapshot(sessionId, token, managerUrl);
        if (!diff || cancelled) {
          logHydration(cancelled ? 'skip:cancelled' : 'skip:no-diff', {
            sessionId,
            cancelled,
            attempt: hydrationAttemptRef.current,
          });
          if (!cancelled) {
            scheduleRetry('no-diff');
          }
          return;
        }
        const sequenceKey = `${sessionId}:${diff.sequence ?? 0}`;
        if (prehydratedSequenceRef.current === sequenceKey) {
          logHydration('skip:already-applied', {
            sessionId,
            sequence: diff.sequence ?? 0,
          });
          return;
        }
        const hydrated = hydrateTerminalStoreFromDiff(store, diff, {});
        if (hydrated) {
          hydrationAttemptRef.current = 0;
          prehydratedSequenceRef.current = sequenceKey;
          cachedStyleUpdatesRef.current = buildStyleUpdates(diff.payload.styles ?? null, diff.sequence ?? 0);
          if (cachedStyleUpdatesRef.current.length > 0) {
            store.applyUpdates(cachedStyleUpdatesRef.current, {
              authoritative: false,
              origin: 'cached-style-refresh',
            });
            logHydration('styles-applied', {
              sessionId,
              sequence: diff.sequence ?? 0,
              count: cachedStyleUpdatesRef.current.length,
            });
          }
          try {
            const snapshot = store.getSnapshot();
            logHydration('applied-cached-diff', {
              sessionId,
              sequence: diff.sequence ?? 0,
              rows: snapshot.rows.length,
              baseRow: snapshot.baseRow,
              viewportTop: snapshot.viewportTop,
              viewportHeight: snapshot.viewportHeight,
            });
          } catch (error) {
            logHydration('applied-cached-diff', {
              sessionId,
              sequence: diff.sequence ?? 0,
              snapshotError: error instanceof Error ? error.message : String(error),
            });
          }
        } else {
          logHydration('hydrate-returned-false', {
            sessionId,
            sequence: diff.sequence ?? 0,
          });
          scheduleRetry('hydrate-false');
        }
      } catch (error) {
        logHydration('fetch:error', {
          sessionId,
          error: error instanceof Error ? error.message : String(error),
        });
        scheduleRetry('fetch-error');
      }
    };
    fetchAndHydrate();
    return () => {
      cancelled = true;
      if (hydrationRetryTimerRef.current != null && typeof window !== 'undefined') {
        window.clearTimeout(hydrationRetryTimerRef.current);
      }
      hydrationRetryTimerRef.current = null;
    };
  }, [managerToken, managerUrl, refresh, sessionMeta?.sessionId, viewer.store]);

  useEffect(() => {
    const store = viewer.store;
    if (!store) {
      return undefined;
    }

    const maybeApplyCachedStyles = (reason: string) => {
      const styleUpdates = cachedStyleUpdatesRef.current;
      if (!styleUpdates || styleUpdates.length === 0) {
        return;
      }
      try {
        const snapshot = store.getSnapshot?.();
        const styleCount = snapshot?.styles ? snapshot.styles.size : 0;
        if (styleCount > 1) {
          return;
        }
        store.applyUpdates(styleUpdates, { authoritative: false, origin: 'cached-style-restore' });
        logHydration('styles-restore', {
          sessionId: sessionMeta?.sessionId ?? null,
          reason,
          count: styleUpdates.length,
          previousStyleCount: styleCount,
        });
      } catch (error) {
        logHydration('styles-restore-error', {
          sessionId: sessionMeta?.sessionId ?? null,
          reason,
          error: error instanceof Error ? error.message : String(error),
        });
      }
    };

    maybeApplyCachedStyles('effect-init');

    if (typeof store.subscribe !== 'function') {
      return undefined;
    }
    const unsubscribe = store.subscribe(() => {
      maybeApplyCachedStyles('store-update');
    });
    return () => {
      try {
        unsubscribe();
      } catch (error) {
        logHydration('styles-restore-unsubscribe-error', {
          sessionId: sessionMeta?.sessionId ?? null,
          error: error instanceof Error ? error.message : String(error),
        });
      }
    };
  }, [sessionMeta?.sessionId, viewer.store]);

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
        const nextMeta = sessionSummaryToTileMeta(session);
        onSessionMetaChange?.(nextMeta);
        setCredentialOverride({ passcode: trimmedCode });
        setCodeInput('');
        setSessionIdInput(session.session_id);
        try {
          const metadataPayload = buildSessionMetadataWithTile(session.metadata, tileId, nextMeta);
          await updateSessionRoleById(
            session.session_id,
            'application',
            token,
            managerUrl,
            metadataPayload,
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
