'use client';

import { memo, useCallback, useEffect, useMemo, useState, type CSSProperties } from 'react';
import { useSessionTerminal, type TerminalViewerState } from '../hooks/useSessionTerminal';
import { BeachTerminal, type TerminalViewportState } from '../../../beach-surfer/src/components/BeachTerminal';
import { CabanaPrivateBeachPlayer } from '../../../beach-surfer/src/components/cabana/CabanaPrivateBeachPlayer';
import type { CabanaTelemetryHandlers } from '../../../beach-surfer/src/components/cabana/CabanaSessionPlayer';

const DEFAULT_HOST_COLS = 80;
const DEFAULT_HOST_ROWS = 24;
const TERMINAL_PADDING_X = 48;
const TERMINAL_PADDING_Y = 56;
const BASE_TERMINAL_FONT_SIZE = 14;
const BASE_TERMINAL_CELL_WIDTH = 8;
const MINIMUM_SCALE = 0.05;

function estimateHostPixelSize(cols: number, rows: number, fontSize: number) {
  const devicePixelRatio =
    typeof window !== 'undefined' && typeof window.devicePixelRatio === 'number'
      ? window.devicePixelRatio || 1
      : 1;
  const baseCellWidth = (fontSize / BASE_TERMINAL_FONT_SIZE) * BASE_TERMINAL_CELL_WIDTH;
  const roundedCellWidth = Math.max(1, Math.round(baseCellWidth * devicePixelRatio) / devicePixelRatio);
  const lineHeight = Math.round(fontSize * 1.4);
  const width = cols * roundedCellWidth + TERMINAL_PADDING_X;
  const height = rows * lineHeight + TERMINAL_PADDING_Y;
  return { width, height, cellWidth: roundedCellWidth, lineHeight };
}

export type HostResizeControlState = {
  needsResize: boolean;
  canResize: boolean;
  trigger: () => void;
  viewportRows: number;
  hostViewportRows: number | null;
  viewportCols: number;
  hostCols: number | null;
};

type Props = {
  sessionId: string;
  privateBeachId: string;
  managerUrl: string;
  token: string | null;
  className?: string;
  variant?: 'preview' | 'full';
  harnessType?: string | null;
  onHostResizeStateChange?: (sessionId: string, state: HostResizeControlState | null) => void;
  fontSize?: number;
  scale?: number;
  locked?: boolean;
  cropped?: boolean;
  targetSize?: { width: number; height: number } | null;
  onViewportDimensions?: (
    sessionId: string,
    dims: {
      viewportRows: number;
      viewportCols: number;
      hostRows: number | null;
      hostCols: number | null;
    },
  ) => void;
  viewerOverride?: TerminalViewerState | null;
};

type ViewProps = Omit<Props, 'token' | 'viewerOverride'> & {
  viewer: TerminalViewerState;
  trimmedToken: string;
  isCabana: boolean;
};

function SessionTerminalPreviewView({
  sessionId,
  privateBeachId,
  managerUrl,
  className,
  variant = 'preview',
  harnessType: _harnessType,
  onHostResizeStateChange,
  fontSize,
  scale,
  locked = false,
  cropped = false,
  onViewportDimensions,
  viewer,
  trimmedToken,
  isCabana,
  targetSize,
}: ViewProps) {
  const [viewportState, setViewportState] = useState<TerminalViewportState | null>(null);
  const [hostDimensions, setHostDimensions] = useState<{ rows: number | null; cols: number | null }>({
    rows: null,
    cols: null,
  });

  useEffect(() => {
    if (typeof window === 'undefined') return;
    try {
      console.info('[terminal][diag] mount', {
        sessionId,
        variant,
        isCabana,
      });
    } catch {
      // ignore logging issues
    }
    return () => {
      if (typeof window === 'undefined') return;
      try {
        console.info('[terminal][diag] unmount', { sessionId });
      } catch {
        // ignore logging issues
      }
    };
  }, [isCabana, sessionId, variant]);

  useEffect(() => {
    if (typeof window === 'undefined') return;
    try {
      console.info('[terminal][diag] viewer-change', {
        sessionId,
        store: viewer.store,
        transport: viewer.transport,
        status: viewer.status,
        connecting: viewer.connecting,
        latencyMs: viewer.latencyMs,
        hasToken: Boolean(trimmedToken),
      });
    } catch {
      // ignore logging issues
    }
  }, [sessionId, trimmedToken, viewer.connecting, viewer.latencyMs, viewer.status, viewer.store, viewer.transport]);

  const baseFontSize = variant === 'full' ? 14 : 12;
  const effectiveFontSize = useMemo(() => {
    const candidate =
      typeof fontSize === 'number' && Number.isFinite(fontSize) ? fontSize : baseFontSize;
    const clamped = Math.max(8, Math.min(candidate, 28));
    const dpr =
      typeof window !== 'undefined' && typeof window.devicePixelRatio === 'number'
        ? window.devicePixelRatio || 1
        : 1;
    const step = 1 / Math.max(1, Math.round(dpr * 2));
    const normalized = Math.round(clamped / step) * step;
    return Number(normalized.toFixed(3));
  }, [fontSize, baseFontSize]);

  const handleViewportStateChange = useCallback((state: TerminalViewportState) => {
    if (typeof window !== 'undefined') {
      console.info('[terminal] viewport-state', {
        version: 'v1',
        sessionId,
        viewportRows: state.viewportRows,
        viewportCols: state.viewportCols,
        hostViewportRows: state.hostViewportRows,
        hostCols: state.hostCols,
      });
    }
    setViewportState(state);
  }, [sessionId]);

  useEffect(() => {
    if (!viewportState) {
      return;
    }
    setHostDimensions((current) => {
      let rows = current.rows;
      let cols = current.cols;
      let changed = false;
      if (typeof viewportState.hostViewportRows === 'number' && viewportState.hostViewportRows > 0) {
        if (rows !== viewportState.hostViewportRows) {
          rows = viewportState.hostViewportRows;
          changed = true;
        }
      } else if (rows == null && viewportState.viewportRows > 0) {
        rows = viewportState.viewportRows;
        changed = true;
      }
      if (typeof viewportState.hostCols === 'number' && viewportState.hostCols > 0) {
        if (cols !== viewportState.hostCols) {
          cols = viewportState.hostCols;
          changed = true;
        }
      } else if (cols == null && viewportState.viewportCols > 0) {
        cols = viewportState.viewportCols;
        changed = true;
      }
      if (!changed) {
        return current;
      }
      return { rows, cols };
    });
  }, [viewportState]);

  useEffect(() => {
    if (typeof window === 'undefined') return;
    try {
      console.info('[terminal][diag] host-dimensions', {
        sessionId,
        rows: hostDimensions.rows,
        cols: hostDimensions.cols,
        viewportRows: viewportState?.viewportRows ?? null,
        viewportCols: viewportState?.viewportCols ?? null,
      });
    } catch {
      // ignore logging issues
    }
  }, [hostDimensions.cols, hostDimensions.rows, sessionId, viewportState?.viewportCols, viewportState?.viewportRows]);

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

  useEffect(() => {
    if (viewer.store) {
      return;
    }
    setViewportState(null);
    setHostDimensions({ rows: null, cols: null });
    if (typeof window !== 'undefined') {
      try {
        console.info('[terminal][diag] reset-dimensions', { sessionId });
      } catch {
        // ignore logging issues
      }
    }
  }, [sessionId, viewer.store]);

  useEffect(() => {
    if (!onHostResizeStateChange) {
      return;
    }
    if (!viewportState) {
      onHostResizeStateChange(sessionId, null);
      return;
    }
    const needsResize =
      viewportState.hostViewportRows != null &&
      viewportState.viewportRows !== viewportState.hostViewportRows;
    const canResize = viewportState.canSendResize && needsResize;
    onHostResizeStateChange(sessionId, {
      needsResize,
      canResize,
      trigger: viewportState.sendHostResize,
      viewportRows: viewportState.viewportRows,
      hostViewportRows: viewportState.hostViewportRows,
      viewportCols: viewportState.viewportCols,
      hostCols: viewportState.hostCols,
    });
  }, [onHostResizeStateChange, sessionId, viewportState]);

  useEffect(() => {
    if (!onViewportDimensions || !viewportState) {
      return;
    }
    if (typeof window !== 'undefined') {
      console.info(
        '[terminal] viewport-dims dispatch',
        JSON.stringify({
          version: 'v1',
          sessionId,
          viewportRows: viewportState.viewportRows,
          viewportCols: viewportState.viewportCols,
          hostRows: hostDimensions.rows,
          hostCols: hostDimensions.cols,
        }),
      );
    }
    onViewportDimensions(sessionId, {
      viewportRows: viewportState.viewportRows,
      viewportCols: viewportState.viewportCols,
      hostRows: hostDimensions.rows,
      hostCols: hostDimensions.cols,
    });
  }, [hostDimensions, onViewportDimensions, sessionId, viewportState]);

  useEffect(
    () => () => {
      onHostResizeStateChange?.(sessionId, null);
    },
    [onHostResizeStateChange, sessionId],
  );

  const scaleValue =
    typeof scale === 'number' && Number.isFinite(scale) ? Math.max(scale, MINIMUM_SCALE) : null;
  const hostCols =
    hostDimensions.cols && hostDimensions.cols > 0
      ? hostDimensions.cols
      : viewportState?.viewportCols && viewportState.viewportCols > 0
        ? viewportState.viewportCols
        : DEFAULT_HOST_COLS;
  const hostRows =
    hostDimensions.rows && hostDimensions.rows > 0
      ? hostDimensions.rows
      : viewportState?.viewportRows && viewportState.viewportRows > 0
        ? viewportState.viewportRows
        : DEFAULT_HOST_ROWS;
  const hostPixelSize = useMemo(() => {
    return estimateHostPixelSize(hostCols, hostRows, effectiveFontSize);
  }, [hostCols, hostRows, effectiveFontSize]);

  useEffect(() => {
    if (typeof window === 'undefined') return;
    try {
      console.info('[terminal][diag] scale-state', {
        sessionId,
        incomingScale: scale,
        resolvedScale: scaleValue,
        locked,
        cropped,
        targetSize,
        hostCols,
        hostRows,
        fontSize: effectiveFontSize,
      });
    } catch {
      // ignore logging issues
    }
  }, [cropped, effectiveFontSize, hostCols, hostRows, locked, scale, scaleValue, sessionId, targetSize]);

  const scaledWrapperStyle = useMemo(() => {
    if (!scaleValue) {
      return undefined;
    }
    if (typeof window !== 'undefined') {
      try {
        console.info('[terminal] target-size', {
          version: 'v1',
          sessionId,
          targetWidth: targetSize?.width ?? null,
          targetHeight: targetSize?.height ?? null,
          scale: scaleValue,
        });
      } catch {
        // ignore logging errors
      }
    }
    const baseWidth =
      targetSize && targetSize.width > 0
        ? targetSize.width / scaleValue
        : hostPixelSize?.width ?? undefined;
    const baseHeight =
      targetSize && targetSize.height > 0
        ? targetSize.height / scaleValue
        : hostPixelSize?.height ?? undefined;
    const style: CSSProperties = {
      transform: `scale(${scaleValue})`,
      transformOrigin: 'top left',
      width: baseWidth ? `${baseWidth}px` : undefined,
      height: baseHeight ? `${baseHeight}px` : undefined,
    };
    return style;
  }, [hostPixelSize, scaleValue, sessionId, targetSize]);

  useEffect(() => {
    if (
      !scaleValue ||
      !hostPixelSize ||
      typeof window === 'undefined' ||
      isCabana ||
      !trimmedToken ||
      !viewer.store ||
      !viewer.transport
    ) {
      return;
    }
    console.info('[terminal] zoom-wrapper', {
      version: 'v1',
      sessionId,
      scale: scaleValue,
      hostCols,
      hostRows,
      widthPx: Math.round(hostPixelSize.width),
      heightPx: Math.round(hostPixelSize.height),
    });
  }, [
    scaleValue,
    hostPixelSize,
    hostCols,
    hostRows,
    sessionId,
    isCabana,
    trimmedToken,
    viewer.store,
    viewer.transport,
  ]);

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
      <div className="flex h-full w-full items-start justify-start overflow-hidden">
        <div className="origin-top-left" style={scaledWrapperStyle}>
          <BeachTerminal
            store={viewer.store}
            transport={viewer.transport}
            autoConnect={false}
            className="h-full w-full"
            fontSize={effectiveFontSize}
            showTopBar={variant === 'full'}
            showStatusBar={variant === 'full'}
            autoResizeHostOnViewportChange={locked}
            onViewportStateChange={handleViewportStateChange}
            disableViewportMeasurements={Boolean(scaleValue)}
            forcedViewportRows={hostRows}
          />
        </div>
      </div>
      {viewer.status === 'reconnecting' && (
        <div className="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center">
          <span className="rounded-full border border-amber-500/40 bg-amber-500/15 px-3 py-1 text-[11px] font-medium uppercase tracking-[0.24em] text-amber-100">
            Reconnecting…
          </span>
        </div>
      )}
      {cropped && (
        <div className="pointer-events-none absolute right-2 bottom-2 rounded-full border border-amber-400/40 bg-amber-500/15 px-2 py-[2px] text-[10px] uppercase tracking-[0.24em] text-amber-100">
          Cropped
        </div>
      )}
    </div>
  );
}

function SessionTerminalPreviewManaged({
  trimmedToken,
  isCabana,
  token: _token,
  viewerOverride: _viewerOverride,
  ...rest
}: Props & { trimmedToken: string; isCabana: boolean }) {
  const viewer = useSessionTerminal(
    rest.sessionId,
    rest.privateBeachId,
    rest.managerUrl,
    !isCabana && trimmedToken.length > 0 ? trimmedToken : null,
  );
  return (
    <SessionTerminalPreviewView
      {...rest}
      trimmedToken={trimmedToken}
      isCabana={isCabana}
      viewer={viewer}
    />
  );
}

function SessionTerminalPreviewClientInner(props: Props) {
  const trimmedToken = props.token?.trim() ?? '';
  const isCabana = props.harnessType ? props.harnessType.toLowerCase().includes('cabana') : false;

  if (props.viewerOverride) {
    const { token: _token, viewerOverride, ...rest } = props;
    return (
      <SessionTerminalPreviewView
        {...rest}
        trimmedToken={trimmedToken}
        isCabana={isCabana}
        viewer={viewerOverride}
      />
    );
  }

  return (
    <SessionTerminalPreviewManaged
      {...props}
      trimmedToken={trimmedToken}
      isCabana={isCabana}
    />
  );
}

export const SessionTerminalPreviewClient = memo(SessionTerminalPreviewClientInner);
export type SessionTerminalPreviewClientProps = Props;
