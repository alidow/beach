'use client';

import { memo, useCallback, useMemo } from 'react';
import { useSessionTerminal } from '../hooks/useSessionTerminal';
import { BeachTerminal } from '../../../beach-surfer/src/components/BeachTerminal';
import { CabanaPrivateBeachPlayer } from '../../../beach-surfer/src/components/cabana/CabanaPrivateBeachPlayer';
import type { CabanaTelemetryHandlers } from '../../../beach-surfer/src/components/cabana/CabanaSessionPlayer';

type Props = {
  sessionId: string;
  privateBeachId: string;
  managerUrl: string;
  token: string | null;
  className?: string;
  variant?: 'preview' | 'full';
  harnessType?: string | null;
};

function SessionTerminalPreviewClientInner({
  sessionId,
  privateBeachId,
  managerUrl,
  token,
  className,
  variant = 'preview',
  harnessType,
}: Props) {
  const trimmedToken = token?.trim() ?? '';
  const isCabana = harnessType ? harnessType.toLowerCase().includes('cabana') : false;

  const cabanaTelemetry = useMemo<CabanaTelemetryHandlers>(
    () => ({
      onStateChange: ({ status, mode }) => {
        console.info('[private-beach] cabana state', { sessionId, status, mode });
      },
      onFirstFrame: ({ elapsedMs, mode, codec }) => {
        console.info('[private-beach] cabana first frame', {
          sessionId,
          elapsedMs: Math.round(elapsedMs),
          mode,
          codec,
        });
      },
      onError: ({ message }) => {
        console.warn('[private-beach] cabana viewer error', { sessionId, message });
      },
    }),
    [sessionId],
  );

  const cabanaSignedOut = useMemo(
    () => (
      <div
        className={
          variant === 'preview'
            ? 'flex h-full w-full items-center justify-center bg-neutral-950/90 text-xs text-muted-foreground'
            : 'flex h-full w-full items-center justify-center bg-neutral-950 text-sm text-muted-foreground'
        }
      >
        <span>Sign in to stream this Cabana session.</span>
      </div>
    ),
    [variant],
  );

  const cabanaLoading = useMemo(
    () => (
      <div
        className={
          variant === 'preview'
            ? 'flex h-full w-full items-center justify-center bg-neutral-950/90 text-xs text-muted-foreground'
            : 'flex h-full w-full items-center justify-center bg-neutral-950 text-sm text-muted-foreground'
        }
      >
        <span>Preparing Cabana stream…</span>
      </div>
    ),
    [variant],
  );

  const cabanaCredentialError = useCallback(
    (message: string) => (
      <div
        className={
          variant === 'preview'
            ? 'flex h-full w-full items-center justify-center bg-neutral-950/90 px-4 text-center text-xs text-red-300'
            : 'flex h-full w-full items-center justify-center bg-neutral-950 px-6 text-center text-sm text-red-300'
        }
      >
        <span>{message}</span>
      </div>
    ),
    [variant],
  );

  const cabanaClassName = useMemo(
    () =>
      [
        variant === 'preview'
          ? 'h-full w-full overflow-hidden bg-neutral-950/90'
          : 'h-full w-full bg-neutral-950',
        className,
      ]
        .filter(Boolean)
        .join(' '),
    [variant, className],
  );

  if (isCabana) {
    return (
      <CabanaPrivateBeachPlayer
        sessionId={sessionId}
        privateBeachId={privateBeachId}
        managerUrl={managerUrl}
        authToken={trimmedToken}
        className={cabanaClassName}
        telemetry={cabanaTelemetry}
        signedOutState={cabanaSignedOut}
        loadingState={cabanaLoading}
        credentialErrorState={cabanaCredentialError}
      />
    );
  }

  if (!trimmedToken) {
    return (
      <div
        className={
          variant === 'preview'
            ? `flex h-full items-center justify-center bg-neutral-950/90 text-xs text-muted-foreground ${className ?? ''}`
            : `flex h-full items-center justify-center bg-neutral-950 text-sm text-muted-foreground ${className ?? ''}`
        }
      >
        <span>Sign in to stream this session.</span>
      </div>
    );
  }

  const viewer = useSessionTerminal(sessionId, privateBeachId, managerUrl, trimmedToken);

  const placeholderMessage = useMemo(() => {
    if (viewer.status === 'error') {
      return viewer.error ?? 'Unable to connect to this session.';
    }
    if (viewer.status === 'connecting') {
      return 'Connecting…';
    }
    if (viewer.status === 'reconnecting') {
      return 'Reconnecting…';
    }
    if (!viewer.transport) {
      return 'Viewer unavailable';
    }
    return null;
  }, [viewer.error, viewer.status, viewer.transport]);

  const placeholderClass =
    variant === 'preview'
      ? 'flex h-full items-center justify-center bg-neutral-950/90 text-xs text-muted-foreground'
      : 'flex h-full items-center justify-center bg-neutral-950 text-sm text-muted-foreground';

  if (placeholderMessage || !viewer.store || !viewer.transport) {
    const merged = className ? `${placeholderClass} ${className}` : placeholderClass;
    return (
      <div className={merged}>
        <span>{placeholderMessage ?? 'Viewer unavailable'}</span>
      </div>
    );
  }

  const containerClass =
    variant === 'preview'
      ? ['relative h-full w-full overflow-hidden bg-neutral-950/90', className]
          .filter(Boolean)
          .join(' ')
      : ['relative h-full w-full bg-neutral-950', className].filter(Boolean).join(' ');

  const secureMode = viewer.secureSummary?.mode === 'secure';
  const secureLabel = secureMode ? 'Secure' : 'Plaintext';
  const secureClass = secureMode
    ? 'border border-emerald-500/40 bg-emerald-500/15 text-emerald-100'
    : 'border border-amber-500/40 bg-amber-500/15 text-amber-100';

  let latencyLabel = 'Latency —';
  let latencyClass = 'border border-slate-600/40 bg-slate-900/70 text-slate-200';
  if (viewer.latencyMs != null) {
    if (viewer.latencyMs >= 1000) {
      latencyLabel = `Latency ${(viewer.latencyMs / 1000).toFixed(1)}s`;
    } else {
      latencyLabel = `Latency ${Math.round(viewer.latencyMs)}ms`;
    }
    if (viewer.latencyMs < 150) {
      latencyClass = 'border border-emerald-500/30 bg-emerald-500/10 text-emerald-100';
    } else if (viewer.latencyMs < 400) {
      latencyClass = 'border border-amber-500/30 bg-amber-500/10 text-amber-100';
    } else {
      latencyClass = 'border border-rose-500/40 bg-rose-500/15 text-rose-100';
    }
  }

  const overlayTextClass = variant === 'full' ? 'text-[11px]' : 'text-[10px]';

  return (
    <div className={containerClass}>
      <div className="pointer-events-none absolute left-2 top-2 flex flex-wrap items-center gap-2 font-semibold uppercase tracking-[0.2em]">
        <span className={`${overlayTextClass} rounded-full px-3 py-1 ${secureClass}`}>{secureLabel}</span>
        <span className={`${overlayTextClass} rounded-full px-3 py-1 ${latencyClass}`}>{latencyLabel}</span>
      </div>
      <BeachTerminal
        store={viewer.store}
        transport={viewer.transport}
        autoConnect={false}
        className="h-full w-full"
        fontSize={variant === 'full' ? 14 : 12}
        showTopBar={variant === 'full'}
        showStatusBar={variant === 'full'}
      />
      {viewer.status === 'reconnecting' && (
        <div className="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center">
          <span className="rounded-full border border-amber-500/40 bg-amber-500/15 px-3 py-1 text-[11px] font-medium uppercase tracking-[0.24em] text-amber-100">
            Reconnecting…
          </span>
        </div>
      )}
    </div>
  );
}

export const SessionTerminalPreviewClient = memo(SessionTerminalPreviewClientInner);
export type SessionTerminalPreviewClientProps = Props;
