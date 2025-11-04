'use client';

import { useCallback, useEffect, useMemo, useState, type FormEvent } from 'react';
import type { SessionSummary } from '@private-beach/shared-api';
import { attachByCode, updateSessionRoleById } from '@private-beach-rewrite/lib/api';
import type { TileSessionMeta } from '@private-beach-rewrite/features/tiles';
import type { SessionCredentialOverride } from '../../../private-beach/src/hooks/terminalViewerTypes';
import { useManagerToken, buildManagerUrl } from '../hooks/useManagerToken';
import { useSessionConnection } from '../hooks/useSessionConnection';
import { SessionViewer } from './SessionViewer';

type ApplicationTileProps = {
  tileId: string;
  privateBeachId: string;
  managerUrl?: string;
  sessionMeta?: TileSessionMeta | null;
  onSessionMetaChange?: (meta: TileSessionMeta | null) => void;
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

  return (
    <div className="application-tile">
      <div className={`application-tile__status application-tile__status--${connectionTone}`}>
        <span>{connectionLabel}</span>
        {viewer.latencyMs != null && viewer.latencyMs > 0 && (
          <span className="application-tile__status-latency">{Math.round(viewer.latencyMs)} ms</span>
        )}
      </div>

      {!hasSession ? (
        <form className="application-tile__form" onSubmit={handleAttach}>
          <label>
            <span>Session ID</span>
            <input
              value={sessionIdInput}
              onChange={(event) => setSessionIdInput(event.target.value)}
              placeholder="sess-1234…"
              autoComplete="off"
            />
          </label>
          <label>
            <span>Passcode</span>
            <input
              value={codeInput}
              onChange={(event) => setCodeInput(event.target.value)}
              placeholder="6-digit code"
              autoComplete="off"
            />
          </label>
          <button type="submit" disabled={disabled}>
            {submitState === 'attaching' ? 'Attaching…' : 'Connect'}
          </button>
          {!isLoaded && <p className="application-tile__hint">Loading authentication…</p>}
          {isLoaded && !isSignedIn && (
            <p className="application-tile__hint">Sign in with Clerk to request manager credentials.</p>
          )}
          {tokenError && <p className="application-tile__error">{tokenError}</p>}
          {attachError && <p className="application-tile__error">{attachError}</p>}
        </form>
      ) : (
        <div className="application-tile__connected">
          <div className="application-tile__summary">
            <div className="application-tile__summary-info">
              <strong title={sessionMeta?.title ?? sessionMeta?.sessionId ?? undefined}>
                {sessionMeta?.title ?? sessionMeta?.sessionId ?? 'Attached Session'}
              </strong>
              <span className="application-tile__summary-subtitle">
                {sessionMeta?.harnessType ?? 'Unknown harness'}
              </span>
            </div>
            <button type="button" onClick={handleDisconnect}>
              Disconnect
            </button>
          </div>
          {roleWarning && <p className="application-tile__warning">{roleWarning}</p>}
          {attachError && <p className="application-tile__error application-tile__error--inline">{attachError}</p>}
          {viewer.status === 'error' && viewer.error && (
            <p className="application-tile__error application-tile__error--inline">{viewer.error}</p>
          )}
          <div className="application-tile__preview">
            <SessionViewer viewer={viewer} sessionId={sessionMeta?.sessionId ?? null} />
          </div>
        </div>
      )}
    </div>
  );
}
