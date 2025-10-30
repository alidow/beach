'use client';

import { memo, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import {
  useSessionTerminal,
  type SessionCredentialOverride,
  type TerminalViewerState,
} from '../hooks/useSessionTerminal';
import { BeachTerminal, type TerminalViewportState } from '../../../beach-surfer/src/components/BeachTerminal';
import { CabanaPrivateBeachPlayer } from '../../../beach-surfer/src/components/cabana/CabanaPrivateBeachPlayer';
import type { CabanaTelemetryHandlers } from '../../../beach-surfer/src/components/cabana/CabanaSessionPlayer';
import {
  hydrateTerminalStoreFromDiff,
  type TerminalStateDiff,
} from '../lib/terminalHydrator';

const DEFAULT_HOST_COLS = 80;
const DEFAULT_HOST_ROWS = 24;
const TERMINAL_PADDING_X = 48;
const TERMINAL_PADDING_Y = 56;
const BASE_TERMINAL_FONT_SIZE = 14;
const BASE_TERMINAL_CELL_WIDTH = 8;
const MINIMUM_SCALE = 0.05;
const MAX_PREVIEW_WIDTH = 450;
const MAX_PREVIEW_HEIGHT = 450;

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
  request?: (opts: { rows: number; cols?: number }) => void;
  viewportRows: number;
  hostViewportRows: number | null;
  viewportCols: number;
  hostCols: number | null;
};

type PreviewStatus = 'connecting' | 'initializing' | 'ready' | 'error';

type PreviewMeasurements = {
  scale: number;
  targetWidth: number;
  targetHeight: number;
  rawWidth: number;
  rawHeight: number;
  hostRows: number | null;
  hostCols: number | null;
  measurementVersion: number;
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
  onViewportDimensions?: (
    sessionId: string,
    dims: {
      viewportRows: number;
      viewportCols: number;
      hostRows: number | null;
      hostCols: number | null;
    },
  ) => void;
  onPreviewStatusChange?: (status: PreviewStatus) => void;
  onPreviewMeasurementsChange?: (sessionId: string, measurements: PreviewMeasurements | null) => void;
  credentialOverride?: SessionCredentialOverride | null;
  viewerOverride?: TerminalViewerState | null;
  cachedStateDiff?: TerminalStateDiff | undefined;
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
  credentialOverride,
  onViewportDimensions,
  onPreviewStatusChange,
  onPreviewMeasurementsChange,
  viewer,
  trimmedToken,
  isCabana,
  cachedStateDiff,
}: ViewProps) {
  const [viewportState, setViewportState] = useState<TerminalViewportState | null>(null);
  const [hostDimensions, setHostDimensions] = useState<{ rows: number | null; cols: number | null }>({
    rows: null,
    cols: null,
  });
  const cloneWrapperRef = useRef<HTMLDivElement | null>(null);
  const cloneInnerRef = useRef<HTMLDivElement | null>(null);
  const previewStatusRef = useRef<PreviewStatus>('connecting');
  const [previewStatus, setPreviewStatusState] = useState<PreviewStatus>('connecting');
  const measurementsRef = useRef<PreviewMeasurements | null>(null);
  const measurementVersionRef = useRef<number>(1);
  const domRawSizeRef = useRef<{ width: number; height: number } | null>(null);
  const [domRawVersion, setDomRawVersion] = useState(0);
  const [isCloneVisible, setIsCloneVisible] = useState<boolean>(true);
  const prehydratedSeqRef = useRef<number | null>(null);

  const updatePreviewStatus = useCallback(
    (next: PreviewStatus) => {
      if (previewStatusRef.current === next) {
        return;
      }
      previewStatusRef.current = next;
      setPreviewStatusState(next);
      onPreviewStatusChange?.(next);
    },
    [onPreviewStatusChange],
  );

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

  // Observe clone visibility to throttle rendering when off-screen
  useEffect(() => {
    if (typeof window === 'undefined') return;
    const target = cloneWrapperRef.current;
    if (!target || typeof IntersectionObserver === 'undefined') {
      setIsCloneVisible(true);
      return;
    }
    const observer = new IntersectionObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      setIsCloneVisible(entry.isIntersecting && entry.intersectionRatio > 0.01);
    }, { threshold: [0, 0.01, 0.1] });
    observer.observe(target);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (typeof window === 'undefined') return;
    try {
      console.info('[terminal][diag] viewer-change', {
        sessionId,
        store: viewer.store,
        transport: viewer.transport,
        transportVersion: viewer.transportVersion,
        status: viewer.status,
        connecting: viewer.connecting,
        latencyMs: viewer.latencyMs,
        hasToken: Boolean(trimmedToken),
      });
    } catch {
      // ignore logging issues
    }
  }, [
    sessionId,
    trimmedToken,
    viewer.connecting,
    viewer.latencyMs,
    viewer.status,
    viewer.store,
    viewer.transport,
    viewer.transportVersion,
  ]);

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

  const handleViewportStateChange = useCallback(
    (state: TerminalViewportState) => {
      if (typeof window !== 'undefined') {
        console.info('[terminal] viewport-state', {
          version: 'v2',
          sessionId,
          viewportRows: state.viewportRows,
          viewportCols: state.viewportCols,
          hostViewportRows: state.hostViewportRows,
          hostCols: state.hostCols,
        });
      }
      setViewportState(state);
    },
    [sessionId],
  );

  useEffect(() => {
    if (!viewportState) {
      return;
    }
    setHostDimensions((current) => {
      let rows = current.rows;
      let cols = current.cols;
      let changed = false;
      const hostViewportRows =
        typeof viewportState.hostViewportRows === 'number' && viewportState.hostViewportRows > 0
          ? viewportState.hostViewportRows
          : null;
      const measuredViewportRows = viewportState.viewportRows > 0 ? viewportState.viewportRows : null;
      const hostViewportCols =
        typeof viewportState.hostCols === 'number' && viewportState.hostCols > 0
          ? viewportState.hostCols
          : null;
      const measuredViewportCols = viewportState.viewportCols > 0 ? viewportState.viewportCols : null;
      if (hostViewportRows != null && rows !== hostViewportRows) {
        rows = hostViewportRows;
        changed = true;
      } else if (rows == null && measuredViewportRows != null) {
        rows = measuredViewportRows;
        changed = true;
      }
      if (hostViewportCols != null && cols !== hostViewportCols) {
        cols = hostViewportCols;
        changed = true;
      } else if (cols == null && measuredViewportCols != null) {
        cols = measuredViewportCols;
        changed = true;
      }
      if (!changed) {
        return current;
      }
      // bump measurement version when host metadata changes
      measurementVersionRef.current = (measurementVersionRef.current % 1_000_000) + 1;
      if (typeof window !== 'undefined') {
        try {
          console.info('[terminal][trace] host-dimension-update', {
            sessionId,
            prevRows: current.rows,
            nextRows: rows,
            prevCols: current.cols,
            nextCols: cols,
            hostViewportRows,
            measuredViewportRows,
            hostViewportCols,
            measuredViewportCols,
            measurementVersion: measurementVersionRef.current,
          });
        } catch {
          // ignore logging failures
        }
      }
      return { rows, cols };
    });
  }, [sessionId, viewportState]);

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
    prehydratedSeqRef.current = null;
  }, [viewer.store]);

  useEffect(() => {
    if (!viewer.store || !cachedStateDiff) {
      return;
    }
    const seq = cachedStateDiff.sequence ?? 0;
    if (prehydratedSeqRef.current === seq) {
      return;
    }
    const hydrated = hydrateTerminalStoreFromDiff(viewer.store, cachedStateDiff, {
      viewportRows: hostDimensions.rows ?? undefined,
    });
    if (hydrated) {
      prehydratedSeqRef.current = seq;
      if (typeof window !== 'undefined') {
        console.info('[terminal][hydrate] applied cached diff', {
          sessionId,
          sequence: seq,
          rows: cachedStateDiff.payload?.rows ?? null,
          cols: cachedStateDiff.payload?.cols ?? null,
        });
      }
    } else if (typeof window !== 'undefined') {
      console.warn('[terminal][hydrate] failed to apply cached diff', {
        sessionId,
        sequence: seq,
      });
    }
  }, [cachedStateDiff, hostDimensions.rows, sessionId, viewer.store]);

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
      request: viewportState.requestHostResize,
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
    const viewportRowsValue = viewportState.viewportRows > 0 ? viewportState.viewportRows : 0;
    const viewportColsValue = viewportState.viewportCols > 0 ? viewportState.viewportCols : 0;
    const limitedViewportRows =
      hostDimensions.rows != null && viewportRowsValue > 0
        ? Math.min(hostDimensions.rows, viewportRowsValue)
        : viewportRowsValue;
    const limitedViewportCols =
      hostDimensions.cols != null && viewportColsValue > 0
        ? Math.min(hostDimensions.cols, viewportColsValue)
        : viewportColsValue;
    if (typeof window !== 'undefined') {
      try {
        console.info('[terminal][trace] viewport-dims limited', {
          sessionId,
          limitedViewportRows,
          limitedViewportCols,
          rawViewportRows: viewportState.viewportRows,
          rawViewportCols: viewportState.viewportCols,
          hostRows: hostDimensions.rows,
          hostCols: hostDimensions.cols,
        });
      } catch {
        // ignore logging issues
      }
    }
    onViewportDimensions(sessionId, {
      viewportRows: limitedViewportRows,
      viewportCols: limitedViewportCols,
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

  const zoomMultiplier =
    typeof scale === 'number' && Number.isFinite(scale) ? Math.max(scale, MINIMUM_SCALE) : 1;

  const hostViewportRows =
    viewportState?.hostViewportRows && viewportState.hostViewportRows > 0
      ? viewportState.hostViewportRows
      : null;
  const measuredViewportRows =
    viewportState?.viewportRows && viewportState.viewportRows > 0
      ? viewportState.viewportRows
      : null;
  const hostViewportCols =
    viewportState?.hostCols && viewportState.hostCols > 0 ? viewportState.hostCols : null;
  const measuredViewportCols =
    viewportState?.viewportCols && viewportState.viewportCols > 0
      ? viewportState.viewportCols
      : null;

  const resolvedHostRows =
    hostDimensions.rows && hostDimensions.rows > 0 ? hostDimensions.rows : hostViewportRows;
  const resolvedHostCols =
    hostDimensions.cols && hostDimensions.cols > 0 ? hostDimensions.cols : hostViewportCols;

  const fallbackHostRows = resolvedHostRows ?? measuredViewportRows ?? DEFAULT_HOST_ROWS;
  const fallbackHostCols = resolvedHostCols ?? measuredViewportCols ?? DEFAULT_HOST_COLS;

  const hostPixelSize = useMemo(() => {
    return estimateHostPixelSize(fallbackHostCols, fallbackHostRows, effectiveFontSize);
  }, [fallbackHostCols, fallbackHostRows, effectiveFontSize]);

  const previewMeasurements = useMemo<PreviewMeasurements | null>(() => {
    if (
      resolvedHostCols == null ||
      resolvedHostRows == null ||
      resolvedHostCols <= 0 ||
      resolvedHostRows <= 0
    ) {
      return null;
    }
    const domRaw = domRawSizeRef.current;
    let rawWidth = hostPixelSize.width;
    let rawHeight = hostPixelSize.height;
    if (
      domRaw &&
      Number.isFinite(domRaw.width) &&
      Number.isFinite(domRaw.height) &&
      domRaw.width > 0 &&
      domRaw.height > 0
    ) {
      rawWidth = domRaw.width;
      rawHeight = domRaw.height;
    }
    if (!Number.isFinite(rawWidth) || rawWidth <= 0 || !Number.isFinite(rawHeight) || rawHeight <= 0) {
      return null;
    }
    const widthScale = rawWidth > 0 ? MAX_PREVIEW_WIDTH / rawWidth : 1;
    const heightScale = rawHeight > 0 ? MAX_PREVIEW_HEIGHT / rawHeight : 1;
    const limitedScale = Math.min(1, widthScale, heightScale);
    const normalizedScale =
      Number.isFinite(limitedScale) && limitedScale > 0 ? Number(limitedScale.toFixed(6)) : 1;
    const targetWidth = Math.max(1, Math.round(rawWidth * normalizedScale));
    const targetHeight = Math.max(1, Math.round(rawHeight * normalizedScale));
    return {
      scale: normalizedScale,
      targetWidth,
      targetHeight,
      rawWidth: Math.round(rawWidth),
      rawHeight: Math.round(rawHeight),
      hostRows: resolvedHostRows,
      hostCols: resolvedHostCols,
      measurementVersion: measurementVersionRef.current,
    };
  }, [domRawVersion, hostPixelSize.height, hostPixelSize.width, resolvedHostCols, resolvedHostRows]);

  const effectiveScale = useMemo(() => {
    if (!previewMeasurements) {
      return zoomMultiplier;
    }
    return previewMeasurements.scale * zoomMultiplier;
  }, [previewMeasurements, zoomMultiplier]);

  useEffect(() => {
    const previous = measurementsRef.current;
    const next = previewMeasurements;
    const changed =
      previous?.targetWidth !== next?.targetWidth ||
      previous?.targetHeight !== next?.targetHeight ||
      previous?.scale !== next?.scale ||
      previous?.rawWidth !== next?.rawWidth ||
      previous?.rawHeight !== next?.rawHeight ||
      previous?.measurementVersion !== next?.measurementVersion;
    if (!changed) {
      return;
    }
    measurementsRef.current = next ?? null;
    if (typeof window !== 'undefined') {
      try {
        console.info('[terminal][trace] preview-measurements', {
          sessionId,
          measurement: next,
        });
      } catch {
        // ignore logging errors
      }
    }
    onPreviewMeasurementsChange?.(sessionId, next ?? null);
  }, [onPreviewMeasurementsChange, previewMeasurements, sessionId]);

  useEffect(() => {
    if (viewer.status === 'error') {
      updatePreviewStatus('error');
      return;
    }
    if (viewer.status === 'connecting' || viewer.status === 'reconnecting' || viewer.status === 'idle') {
      updatePreviewStatus('connecting');
      return;
    }
    if (viewer.status === 'connected') {
      if (previewMeasurements) {
        updatePreviewStatus('ready');
      } else {
        updatePreviewStatus('initializing');
      }
    }
  }, [previewMeasurements, updatePreviewStatus, viewer.status]);

  useEffect(() => {
    if (!viewer.store) {
      return;
    }
    const ensurePinnedViewport = () => {
      const snapshot = viewer.store!.getSnapshot();
      const desiredTop = snapshot.baseRow;
      const hostRowCount = resolvedHostRows ?? hostViewportRows ?? DEFAULT_HOST_ROWS;
      const desiredHeight = Math.max(1, hostRowCount);
      let changed = false;
      if (snapshot.followTail) {
        viewer.store!.setFollowTail(false);
        changed = true;
      }
      if (snapshot.viewportTop !== desiredTop || snapshot.viewportHeight !== desiredHeight) {
        viewer.store!.setViewport(desiredTop, desiredHeight);
        changed = true;
      }
      if (changed && typeof window !== 'undefined') {
        try {
          console.info('[terminal][trace] viewport-clamped', {
            sessionId,
            desiredTop,
            desiredHeight,
            snapshotTop: snapshot.viewportTop,
            snapshotHeight: snapshot.viewportHeight,
          });
        } catch {
          // ignore logging issues
        }
      }
    };
    ensurePinnedViewport();
    const unsubscribe = viewer.store.subscribe(() => {
      ensurePinnedViewport();
    });
    return unsubscribe;
  }, [hostViewportRows, resolvedHostRows, sessionId, viewer.store]);

  useEffect(() => {
    if (typeof window === 'undefined') return;
    try {
      console.info('[terminal][diag] scale-state', {
        sessionId,
        incomingScale: scale,
        zoomMultiplier,
        effectiveScale,
        locked,
        cropped,
        previewScale: previewMeasurements?.scale ?? null,
        targetWidth: previewMeasurements?.targetWidth ?? null,
        targetHeight: previewMeasurements?.targetHeight ?? null,
        resolvedHostCols,
        resolvedHostRows,
        fallbackHostCols,
        fallbackHostRows,
        fontSize: effectiveFontSize,
      });
    } catch {
      // ignore logging issues
    }
  }, [
    cropped,
    effectiveFontSize,
    effectiveScale,
    fallbackHostCols,
    fallbackHostRows,
    locked,
    previewMeasurements,
    resolvedHostCols,
    resolvedHostRows,
    scale,
    sessionId,
    zoomMultiplier,
  ]);

  useEffect(() => {
    if (typeof window === 'undefined') return;
    const node = cloneWrapperRef.current;
    if (!node || !previewMeasurements) {
      return;
    }
    const logDimensions = () => {
      const rect = node.getBoundingClientRect();
      const child =
        cloneInnerRef.current instanceof HTMLElement ? cloneInnerRef.current.getBoundingClientRect() : null;
      console.info('[terminal][trace] dom-dimensions', {
        sessionId,
        effectiveScale,
        targetWidth: previewMeasurements.targetWidth,
        targetHeight: previewMeasurements.targetHeight,
        wrapperWidth: Math.round(rect.width),
        wrapperHeight: Math.round(rect.height),
        childWidth: child ? Math.round(child.width) : null,
        childHeight: child ? Math.round(child.height) : null,
      });
      const measuredWidth = child ? child.width : rect.width;
      const measuredHeight = child ? child.height : rect.height;
      if (effectiveScale > 0 && Number.isFinite(measuredWidth) && Number.isFinite(measuredHeight)) {
        const rawWidthFromDom = measuredWidth / effectiveScale;
        const rawHeightFromDom = measuredHeight / effectiveScale;
        const prev = domRawSizeRef.current;
        const widthDelta = !prev ? Number.POSITIVE_INFINITY : Math.abs(prev.width - rawWidthFromDom);
        const heightDelta = !prev ? Number.POSITIVE_INFINITY : Math.abs(prev.height - rawHeightFromDom);
        if (!prev || widthDelta > 1 || heightDelta > 1) {
          domRawSizeRef.current = {
            width: rawWidthFromDom,
            height: rawHeightFromDom,
          };
          setDomRawVersion((version) => (version + 1) % 1_000_000);
        }
      }
    };
    const handle = window.requestAnimationFrame(logDimensions);
    return () => window.cancelAnimationFrame(handle);
  }, [effectiveScale, previewMeasurements, sessionId]);

  useEffect(() => {
    if (
      !previewMeasurements ||
      typeof window === 'undefined' ||
      isCabana ||
      !trimmedToken ||
      !viewer.store
    ) {
      return;
    }
    try {
      console.info('[terminal] zoom-wrapper', {
        version: 'v2',
        sessionId,
        zoomMultiplier,
        effectiveScale,
        previewScale: previewMeasurements.scale,
        hostCols: resolvedHostCols ?? fallbackHostCols,
        hostRows: resolvedHostRows ?? fallbackHostRows,
        rawWidth: previewMeasurements.rawWidth,
        rawHeight: previewMeasurements.rawHeight,
        targetWidth: previewMeasurements.targetWidth,
        targetHeight: previewMeasurements.targetHeight,
      });
    } catch {
      // ignore logging issues
    }
  }, [
    effectiveScale,
    fallbackHostCols,
    fallbackHostRows,
    isCabana,
    previewMeasurements,
    resolvedHostCols,
    resolvedHostRows,
    sessionId,
    trimmedToken,
    viewer.store,
    zoomMultiplier,
  ]);

  const driverWrapperStyle = useMemo<CSSProperties>(
    () => ({
      position: 'absolute',
      width: 0,
      height: 0,
      overflow: 'hidden',
      opacity: 0,
      pointerEvents: 'none',
      contain: 'size',
    }),
    [],
  );

  const cloneWrapperStyle = useMemo<CSSProperties | undefined>(() => {
    if (!previewMeasurements) {
      return undefined;
    }
    const width = previewMeasurements.rawWidth * effectiveScale;
    const height = previewMeasurements.rawHeight * effectiveScale;
    return {
      width: `${Math.max(1, Math.round(width))}px`,
      height: `${Math.max(1, Math.round(height))}px`,
    };
  }, [effectiveScale, previewMeasurements]);

  const cloneInnerStyle = useMemo<CSSProperties | undefined>(() => {
    if (!previewMeasurements) {
      return undefined;
    }
    return {
      width: `${previewMeasurements.rawWidth}px`,
      height: `${previewMeasurements.rawHeight}px`,
      transform: `scale(${effectiveScale})`,
      transformOrigin: 'top left',
    };
  }, [effectiveScale, previewMeasurements]);

  const placeholderMessage = useMemo(() => {
    switch (previewStatus) {
      case 'connecting':
        return 'Connecting to session…';
      case 'initializing':
        return 'Preparing terminal preview…';
      case 'error':
        return viewer.error ?? 'Unable to load this session.';
      default:
        return null;
    }
  }, [previewStatus, viewer.error]);

  const hasDirectCredential = useMemo(() => {
    if (!credentialOverride) {
      return false;
    }
    const pass = credentialOverride.passcode?.trim();
    if (pass && pass.length > 0) {
      return true;
    }
    const directViewerToken = credentialOverride.viewerToken?.trim();
    if (directViewerToken && directViewerToken.length > 0) {
      return true;
    }
    return false;
  }, [credentialOverride]);

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

  if (!trimmedToken && !hasDirectCredential) {
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
  const showPlaceholder = previewStatus !== 'ready';
  const overlayClass =
    variant === 'preview'
      ? 'absolute inset-0 flex items-center justify-center bg-neutral-950/90 text-xs text-muted-foreground'
      : 'absolute inset-0 flex items-center justify-center bg-neutral-950 text-sm text-muted-foreground';

  return (
    <div className={containerClass}>
      <div className="pointer-events-none absolute left-2 top-2 flex flex-wrap items-center gap-2 font-semibold uppercase tracking-[0.2em]">
        <span className={`${overlayTextClass} rounded-full px-3 py-1 ${secureClass}`}>{secureLabel}</span>
        <span className={`${overlayTextClass} rounded-full px-3 py-1 ${latencyClass}`}>{latencyLabel}</span>
      </div>
      <div className="relative flex w-full items-start justify-start overflow-hidden">
        <div style={driverWrapperStyle} aria-hidden>
          <BeachTerminal
            store={viewer.store ?? undefined}
            transport={viewer.transport ?? undefined}
            transportVersion={viewer.transportVersion}
            autoConnect={false}
            className="w-full"
            fontSize={effectiveFontSize}
            showTopBar={false}
            showStatusBar={false}
            autoResizeHostOnViewportChange={false}
            onViewportStateChange={handleViewportStateChange}
            disableViewportMeasurements
            maxRenderFps={20}
            hideIdlePlaceholder
          />
        </div>
        <div
          ref={cloneWrapperRef}
          className="relative flex items-start justify-start overflow-hidden"
          style={cloneWrapperStyle}
        >
          <div ref={cloneInnerRef} className="origin-top-left" style={cloneInnerStyle}>
            <BeachTerminal
              store={viewer.store ?? undefined}
              transport={undefined}
              autoConnect={false}
              className="w-full"
              fontSize={effectiveFontSize}
              showTopBar={variant === 'full'}
              showStatusBar={variant === 'full'}
              autoResizeHostOnViewportChange={locked}
              disableViewportMeasurements
              maxRenderFps={isCloneVisible ? undefined : 12}
              hideIdlePlaceholder
            />
          </div>
          {showPlaceholder && (
            <div className={overlayClass}>
              <span>{placeholderMessage ?? 'Preparing terminal preview…'}</span>
            </div>
          )}
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
  credentialOverride,
  token: _token,
  viewerOverride: _viewerOverride,
  ...rest
}: Props & { trimmedToken: string; isCabana: boolean }) {
  const viewer = useSessionTerminal(
    rest.sessionId,
    rest.privateBeachId,
    rest.managerUrl,
    !isCabana && trimmedToken.length > 0 ? trimmedToken : null,
    isCabana ? undefined : credentialOverride ?? undefined,
  );
  return (
    <SessionTerminalPreviewView
      {...rest}
      credentialOverride={credentialOverride}
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
