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
    if (viewer.error) {
      return viewer.error;
    }
    if (viewer.connecting) {
      return viewer.transport ? 'Syncing…' : 'Connecting…';
    }
    if (!viewer.transport) {
      return 'Reconnecting…';
    }
    return null;
  }, [viewer.connecting, viewer.error, viewer.transport]);

  const placeholderClass =
    variant === 'preview'
      ? 'flex h-full items-center justify-center bg-neutral-950/90 text-xs text-muted-foreground'
      : 'flex h-full items-center justify-center bg-neutral-950 text-sm text-muted-foreground';

  if (placeholderMessage) {
    const merged = className ? `${placeholderClass} ${className}` : placeholderClass;
    return (
      <div className={merged}>
        <span>{placeholderMessage}</span>
      </div>
    );
  }

  if (!viewer.store || !viewer.transport) {
    const merged = className ? `${placeholderClass} ${className}` : placeholderClass;
    return (
      <div className={merged}>
        <span>Viewer unavailable</span>
      </div>
    );
  }

  const terminalClass =
    variant === 'preview'
      ? ['h-full w-full overflow-hidden bg-neutral-950/90', className].filter(Boolean).join(' ')
      : ['h-full w-full bg-neutral-950', className].filter(Boolean).join(' ');

  return (
    <BeachTerminal
      store={viewer.store}
      transport={viewer.transport}
      autoConnect={false}
      className={terminalClass}
      fontSize={variant === 'full' ? 14 : 12}
      showTopBar={variant === 'full'}
      showStatusBar={variant === 'full'}
    />
  );
}

export const SessionTerminalPreviewClient = memo(SessionTerminalPreviewClientInner);
export type SessionTerminalPreviewClientProps = Props;
