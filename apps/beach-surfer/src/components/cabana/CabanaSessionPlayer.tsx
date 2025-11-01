'use client';

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Loader2, VideoOff } from 'lucide-react';
import { BeachSessionView, type ViewerMode } from '../BeachSessionView';
import type { TerminalStatus } from '../BeachTerminal';
import type { CabanaCodec } from '../CabanaViewer';
import type { SecureTransportSummary } from '../../transport/webrtc';
import { cn } from '../../lib/utils';

export type CabanaStreamMode = ViewerMode;

export interface CabanaTelemetryHandlers {
  onStateChange?: (event: { status: TerminalStatus; mode: CabanaStreamMode }) => void;
  onFirstFrame?: (event: { elapsedMs: number; mode: Extract<CabanaStreamMode, 'media_png' | 'media_h264'>; codec: CabanaCodec }) => void;
  onError?: (event: { message: string }) => void;
  onSecureSummary?: (summary: SecureTransportSummary | null) => void;
}

export interface CabanaSessionPlayerProps {
  sessionId?: string | null;
  baseUrl?: string | null;
  passcode?: string | null;
  viewerToken?: string | null;
  autoConnect?: boolean;
  clientLabel?: string;
  className?: string;
  emptyState?: ReactNode;
  connectingLabel?: string;
  errorMessage?: string;
  inactiveLabel?: string;
  onStatusChange?: (status: TerminalStatus) => void;
  telemetry?: CabanaTelemetryHandlers;
  showStatusBadges?: boolean;
  viewOnly?: boolean;
}

const STATUS_BADGES: Record<
  TerminalStatus,
  {
    label: string;
    className: string;
    indicator: string;
  }
> = {
  idle: {
    label: 'Idle',
    className: 'bg-slate-900/80 text-slate-200 ring-1 ring-slate-800',
    indicator: 'bg-slate-400',
  },
  connecting: {
    label: 'Connecting',
    className: 'bg-amber-500/10 text-amber-200 ring-1 ring-amber-400/40',
    indicator: 'bg-amber-300',
  },
  connected: {
    label: 'Connected',
    className: 'bg-emerald-500/10 text-emerald-100 ring-1 ring-emerald-400/40',
    indicator: 'bg-emerald-300',
  },
  error: {
    label: 'Error',
    className: 'bg-rose-500/10 text-rose-100 ring-1 ring-rose-400/40',
    indicator: 'bg-rose-300',
  },
  closed: {
    label: 'Ended',
    className: 'bg-slate-900/80 text-slate-200 ring-1 ring-slate-800/70',
    indicator: 'bg-slate-500',
  },
};

const MODE_LABEL: Record<CabanaStreamMode, string> = {
  unknown: 'Awaiting stream…',
  terminal: 'Terminal stream',
  media_png: 'Cabana PNG stream',
  media_h264: 'Cabana H.264 stream',
};

const DEFAULT_EMPTY = (
  <div className="pointer-events-none flex flex-col items-center gap-2 rounded-xl border border-dashed border-slate-700/70 bg-slate-950/70 px-6 py-5 text-center text-sm text-slate-300">
    <VideoOff className="size-6 text-slate-500" />
    <div className="font-medium text-slate-200">Enter a session id to begin</div>
    <p className="text-xs text-slate-400">Provide a Cabana session id and server URL to start streaming.</p>
  </div>
);

const DEFAULT_CONNECTING = (
  <div className="pointer-events-none flex items-center gap-2 rounded-xl border border-slate-800/70 bg-slate-950/80 px-4 py-2 text-sm text-slate-200">
    <Loader2 className="size-4 animate-spin text-amber-300" />
    Negotiating stream…
  </div>
);

const DEFAULT_ERROR = (message: string) => (
  <div className="pointer-events-auto flex max-w-sm flex-col items-center gap-2 rounded-xl border border-rose-500/40 bg-rose-500/10 px-5 py-4 text-center text-sm text-rose-100">
    <span className="font-medium text-rose-100">Unable to load the Cabana session</span>
    <span className="text-xs text-rose-200/80">{message}</span>
  </div>
);

const DEFAULT_INACTIVE = (
  <div className="pointer-events-none flex items-center gap-2 rounded-xl border border-slate-800/70 bg-slate-950/80 px-4 py-2 text-sm text-slate-200">
    Ready to connect
  </div>
);

function emitViewerEvent(event: string, detail: Record<string, unknown>): void {
  if (typeof window === 'undefined') {
    return;
  }
  window.dispatchEvent(new CustomEvent(`cabana-viewer:${event}`, { detail }));
}

export function CabanaSessionPlayer(props: CabanaSessionPlayerProps): JSX.Element {
  const {
    sessionId,
    baseUrl,
    passcode,
    viewerToken,
    autoConnect,
    clientLabel = 'beach-surfer',
    className,
    emptyState = DEFAULT_EMPTY,
    connectingLabel,
    errorMessage = 'Unable to join this Cabana session. Verify the link or passcode and try again.',
    inactiveLabel,
    onStatusChange,
    telemetry,
    showStatusBadges = true,
    viewOnly = false,
  } = props;

  const trimmedSessionId = sessionId?.trim() ?? '';
  const trimmedBaseUrl = baseUrl?.trim() ?? '';
  const trimmedPasscode = passcode?.trim() ?? '';
  const trimmedViewerToken = viewerToken?.trim() ?? '';

  const canAttempt = trimmedSessionId.length > 0 && trimmedBaseUrl.length > 0;
  const shouldAutoConnect = (autoConnect ?? canAttempt) && canAttempt;

  const telemetrySessionId = useMemo(() => {
    if (!trimmedSessionId) return null;
    if (trimmedSessionId.length <= 8) return trimmedSessionId;
    return `${trimmedSessionId.slice(0, 4)}…${trimmedSessionId.slice(-4)}`;
  }, [trimmedSessionId]);

  const telemetryHost = useMemo(() => {
    if (!trimmedBaseUrl) return null;
    try {
      const candidate = /^https?:\/\//i.test(trimmedBaseUrl) ? trimmedBaseUrl : `https://${trimmedBaseUrl}`;
      const url = new URL(candidate);
      return url.host;
    } catch {
      return trimmedBaseUrl;
    }
  }, [trimmedBaseUrl]);

  const [status, setStatus] = useState<TerminalStatus>('idle');
  const [mode, setMode] = useState<CabanaStreamMode>('unknown');
  const [secureSummary, setSecureSummary] = useState<SecureTransportSummary | null>(null);
  const [lastError, setLastError] = useState<string | null>(null);

  const statusRef = useRef<TerminalStatus>(status);
  const modeRef = useRef<CabanaStreamMode>(mode);
  const connectStartedAtRef = useRef<number | null>(null);
  const firstFrameSentRef = useRef<boolean>(false);

  useEffect(() => {
    statusRef.current = status;
  }, [status]);
  useEffect(() => {
    modeRef.current = mode;
  }, [mode]);

  const notifyStateChange = useCallback(
    (nextStatus: TerminalStatus, nextMode: CabanaStreamMode) => {
      telemetry?.onStateChange?.({ status: nextStatus, mode: nextMode });
      emitViewerEvent('state', {
        status: nextStatus,
        mode: nextMode,
        session: telemetrySessionId,
        host: telemetryHost,
      });
    },
    [telemetry, telemetryHost, telemetrySessionId],
  );

  const handleStatusChange = useCallback(
    (next: TerminalStatus) => {
      setStatus(next);
      if (next === 'connecting') {
        connectStartedAtRef.current =
          typeof performance !== 'undefined' ? performance.now() : Date.now();
        firstFrameSentRef.current = false;
        setLastError(null);
      } else if (next === 'error') {
        setLastError(errorMessage);
        telemetry?.onError?.({ message: errorMessage });
        emitViewerEvent('error', {
          message: errorMessage,
          session: telemetrySessionId,
          host: telemetryHost,
        });
      } else if (next === 'idle') {
        connectStartedAtRef.current = null;
        firstFrameSentRef.current = false;
        setLastError(null);
      } else if (next === 'closed') {
        connectStartedAtRef.current = null;
        firstFrameSentRef.current = false;
      }
      onStatusChange?.(next);
      notifyStateChange(next, modeRef.current);
    },
    [errorMessage, notifyStateChange, onStatusChange, telemetry, telemetryHost, telemetrySessionId],
  );

  const handleStreamMode = useCallback(
    (nextMode: CabanaStreamMode) => {
      setMode(nextMode);
      if (nextMode === 'media_png' || nextMode === 'media_h264') {
        if (!firstFrameSentRef.current) {
          firstFrameSentRef.current = true;
          const started = connectStartedAtRef.current;
          const now = typeof performance !== 'undefined' ? performance.now() : Date.now();
          const elapsed = started != null ? Math.max(0, now - started) : 0;
          telemetry?.onFirstFrame?.({
            elapsedMs: elapsed,
            mode: nextMode,
            codec: nextMode === 'media_h264' ? 'media_h264' : 'media_png',
          });
          emitViewerEvent('first-frame', {
            elapsedMs: elapsed,
            mode: nextMode,
            codec: nextMode === 'media_h264' ? 'media_h264' : 'media_png',
            session: telemetrySessionId,
            host: telemetryHost,
          });
        }
      }
      notifyStateChange(statusRef.current, nextMode);
    },
    [notifyStateChange, telemetry, telemetryHost, telemetrySessionId],
  );

  const handleSecureUpdate = useCallback(
    (summary: SecureTransportSummary | null) => {
      setSecureSummary(summary);
      telemetry?.onSecureSummary?.(summary);
      emitViewerEvent('secure', {
        session: telemetrySessionId,
        host: telemetryHost,
        mode: summary?.mode ?? null,
        verificationCode: summary?.verificationCode ?? null,
      });
    },
    [telemetry, telemetryHost, telemetrySessionId],
  );

  useEffect(() => {
    if (!shouldAutoConnect && statusRef.current === 'connecting') {
      setStatus('idle');
    }
  }, [shouldAutoConnect]);

  const overlayContent = useMemo(() => {
    if (!canAttempt) {
      return emptyState;
    }
    if (!shouldAutoConnect) {
      return inactiveLabel ? (
        <div className="pointer-events-none rounded-xl border border-slate-800/70 bg-slate-950/80 px-4 py-2 text-sm text-slate-200">
          {inactiveLabel}
        </div>
      ) : (
        DEFAULT_INACTIVE
      );
    }
    if (status === 'idle' || status === 'connecting') {
      if (connectingLabel) {
        return (
          <div className="pointer-events-none flex items-center gap-2 rounded-xl border border-slate-800/70 bg-slate-950/80 px-4 py-2 text-sm text-slate-200">
            <Loader2 className="size-4 animate-spin text-amber-300" />
            {connectingLabel}
          </div>
        );
      }
      return DEFAULT_CONNECTING;
    }
    if (status === 'error') {
      return DEFAULT_ERROR(lastError ?? errorMessage);
    }
    if (status === 'closed') {
      return (
        <div className="pointer-events-none flex items-center gap-2 rounded-xl border border-slate-800/70 bg-slate-950/80 px-4 py-2 text-sm text-slate-200">
          Stream ended
        </div>
      );
    }
    return null;
  }, [
    canAttempt,
    connectingLabel,
    emptyState,
    errorMessage,
    inactiveLabel,
    lastError,
    shouldAutoConnect,
    status,
  ]);

  const statusBadge = STATUS_BADGES[status];
  const modeLabel = MODE_LABEL[mode];

  return (
    <div className={cn('relative h-full w-full overflow-hidden bg-slate-950', className)}>
      <BeachSessionView
        sessionId={canAttempt ? trimmedSessionId : undefined}
        baseUrl={canAttempt ? trimmedBaseUrl : undefined}
        passcode={trimmedPasscode || undefined}
        viewerToken={trimmedViewerToken || undefined}
        autoConnect={shouldAutoConnect}
        clientLabel={clientLabel}
        onStatusChange={handleStatusChange}
        onStreamKindChange={handleStreamMode}
        onSecureSummary={handleSecureUpdate}
        className="h-full w-full"
        showStatusBar={false}
        showTopBar={false}
        viewOnly={viewOnly}
      />

      <div className="pointer-events-none absolute inset-0">
        {showStatusBadges ? (
          <div className="absolute left-3 top-3 z-20 flex flex-col gap-2">
            <span
              className={cn(
                'inline-flex items-center gap-2 rounded-full px-3 py-1 text-xs font-semibold backdrop-blur',
                statusBadge.className,
              )}
            >
              <span className={cn('size-2 rounded-full', statusBadge.indicator)} />
              {statusBadge.label}
            </span>
            {mode !== 'unknown' ? (
              <span className="inline-flex items-center gap-2 rounded-full border border-slate-800/70 bg-slate-950/70 px-3 py-1 text-[11px] font-medium text-slate-200 backdrop-blur">
                {modeLabel}
              </span>
            ) : null}
          </div>
        ) : null}
        {secureSummary && secureSummary.mode === 'secure' && secureSummary.verificationCode ? (
          <div className="pointer-events-none absolute right-3 top-3 z-20 rounded-xl border border-emerald-400/30 bg-emerald-500/10 px-3 py-1 text-xs text-emerald-100 backdrop-blur">
            Verified • {secureSummary.verificationCode}
          </div>
        ) : null}
        {overlayContent ? (
          <div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center bg-gradient-to-b from-slate-950/40 via-slate-950/50 to-slate-950/30 backdrop-blur-sm transition-opacity">
            {overlayContent}
          </div>
        ) : null}
      </div>
    </div>
  );
}
