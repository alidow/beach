'use client';

import { useEffect, useMemo, useState, type ReactNode } from 'react';
import type { TerminalStatus } from '../BeachTerminal';
import { CabanaSessionPlayer, type CabanaTelemetryHandlers } from './CabanaSessionPlayer';
import { cn } from '../../lib/utils';
import { Loader2, ShieldOff } from 'lucide-react';

interface ViewerCredentialPayload {
  credential_type: string;
  credential: string;
  session_id: string;
  private_beach_id: string;
  passcode?: string | null;
}

interface NormalisedCredential {
  passcode?: string;
  viewerToken: string;
}

type CredentialStatus = 'idle' | 'loading' | 'ready' | 'error';

export interface CabanaPrivateBeachPlayerProps {
  sessionId: string;
  privateBeachId: string;
  managerUrl: string;
  authToken: string | null;
  className?: string;
  autoConnect?: boolean;
  telemetry?: CabanaTelemetryHandlers;
  onStatusChange?: (status: TerminalStatus) => void;
  signedOutState?: ReactNode;
  loadingState?: ReactNode;
  credentialErrorState?: (message: string) => ReactNode;
}

const DEFAULT_SIGNED_OUT = (
  <div className="pointer-events-none flex flex-col items-center gap-2 rounded-lg border border-slate-800/70 bg-slate-950/80 px-5 py-4 text-center text-sm text-slate-200">
    <ShieldOff className="size-5 text-slate-500" />
    <span>Sign in to stream this Cabana session.</span>
  </div>
);

const DEFAULT_LOADING = (
  <div className="pointer-events-none flex items-center gap-2 rounded-lg border border-slate-800/70 bg-slate-950/80 px-4 py-2 text-sm text-slate-200">
    <Loader2 className="size-4 animate-spin text-amber-300" />
    Fetching viewer credential…
  </div>
);

const DEFAULT_ERROR = (message: string) => (
  <div className="pointer-events-auto flex max-w-sm flex-col items-center gap-2 rounded-lg border border-rose-500/40 bg-rose-500/10 px-5 py-4 text-center text-sm text-rose-100">
    <span className="font-medium text-rose-100">Unable to fetch viewer credential</span>
    <span className="text-xs text-rose-200/80">{message}</span>
  </div>
);

function normaliseCredential(payload: ViewerCredentialPayload): NormalisedCredential {
  const type = payload.credential_type?.toLowerCase();
  const credential = payload.credential?.trim() ?? '';
  const passcode = payload.passcode?.toString().trim() ?? '';

  if (type === 'viewer_token') {
    if (!credential) {
      throw new Error('viewer token credential is incomplete');
    }
    return { passcode: passcode || undefined, viewerToken: credential };
  }

  throw new Error(`unsupported viewer credential type: ${payload.credential_type ?? 'unknown'}`);
}

function buildCredentialUrl(baseUrl: string, privateBeachId: string, sessionId: string): string {
  const trimmed = baseUrl.trim().replace(/\/+$/, '');
  const encodedBeach = encodeURIComponent(privateBeachId);
  const encodedSession = encodeURIComponent(sessionId);
  return `${trimmed}/private-beaches/${encodedBeach}/sessions/${encodedSession}/viewer-credential`;
}

export function CabanaPrivateBeachPlayer(props: CabanaPrivateBeachPlayerProps): JSX.Element {
  const {
    sessionId,
    privateBeachId,
    managerUrl,
    authToken,
    className,
    autoConnect = true,
    telemetry,
    onStatusChange,
    signedOutState = DEFAULT_SIGNED_OUT,
    loadingState = DEFAULT_LOADING,
    credentialErrorState = DEFAULT_ERROR,
  } = props;

  const trimmedToken = authToken?.trim() ?? '';
  const trimmedManager = managerUrl?.trim() ?? '';

  const [status, setStatus] = useState<CredentialStatus>('idle');
  const [credential, setCredential] = useState<NormalisedCredential | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  useEffect(() => {
    if (!sessionId || !privateBeachId || !trimmedManager || !trimmedToken) {
      setStatus('idle');
      setCredential(null);
      setErrorMessage(null);
      return;
    }

    let cancelled = false;
    const controller = new AbortController();
    const url = buildCredentialUrl(trimmedManager, privateBeachId, sessionId);
    setStatus('loading');
    setErrorMessage(null);
    setCredential(null);

    (async () => {
      try {
        const response = await fetch(url, {
          headers: {
            Accept: 'application/json',
            'Content-Type': 'application/json',
            Authorization: `Bearer ${trimmedToken}`,
          },
          signal: controller.signal,
        });
        if (!response.ok) {
          if (response.status === 404) {
            throw new Error('This Cabana session is not attached to the selected beach yet.');
          }
          throw new Error(`Viewer credential request failed (${response.status})`);
        }
        const payload: ViewerCredentialPayload = await response.json();
        if (cancelled) {
          return;
        }
        const next = normaliseCredential(payload);
        setCredential(next);
        setStatus('ready');
      } catch (error) {
        if ((error as Error).name === 'AbortError') {
          return;
        }
        console.error('[CabanaPrivateBeachPlayer] credential fetch failed', {
          sessionId,
          privateBeachId,
          managerUrl: trimmedManager,
          error,
        });
        setStatus('error');
        setErrorMessage((error as Error).message || 'Failed to fetch viewer credential.');
        setCredential(null);
      }
    })();

    return () => {
      cancelled = true;
      controller.abort();
    };
  }, [sessionId, privateBeachId, trimmedManager, trimmedToken]);

  const readyToStream = useMemo(() => status === 'ready' && credential !== null, [status, credential]);

  if (!trimmedToken) {
    return (
      <div className={cn('relative flex h-full w-full items-center justify-center bg-slate-950', className)}>
        {signedOutState}
      </div>
    );
  }

  if (!trimmedManager) {
    return (
      <div className={cn('relative flex h-full w-full items-center justify-center bg-slate-950', className)}>
        {credentialErrorState('Private Beach manager URL is not configured.')}
      </div>
    );
  }

  if (status === 'loading' || (status === 'idle' && !credential && autoConnect)) {
    return (
      <div className={cn('relative flex h-full w-full items-center justify-center bg-slate-950', className)}>
        {loadingState}
      </div>
    );
  }

  if (status === 'error' || !readyToStream) {
    return (
      <div className={cn('relative flex h-full w-full items-center justify-center bg-slate-950', className)}>
        {credentialErrorState(errorMessage ?? 'Unable to fetch viewer credential.')}
      </div>
    );
  }

  if (!credential) {
    return (
      <div className={cn('relative flex h-full w-full items-center justify-center bg-slate-950', className)}>
        {credentialErrorState('Viewer credential is unavailable.')}
      </div>
    );
  }

  return (
    <CabanaSessionPlayer
      sessionId={sessionId}
      baseUrl={trimmedManager}
      passcode={credential.passcode}
      viewerToken={credential.viewerToken}
      autoConnect={autoConnect}
      clientLabel="private-beach"
      className={className}
      telemetry={telemetry}
      onStatusChange={onStatusChange}
      emptyState={signedOutState}
      viewOnly
    />
  );
}
