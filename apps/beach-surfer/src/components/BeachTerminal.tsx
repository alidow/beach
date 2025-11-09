import type { CSSProperties, UIEvent } from 'react';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { FEATURE_CURSOR_SYNC } from '../protocol/types';
import type { HostFrame } from '../protocol/types';
import { createTerminalStore, useTerminalSnapshot } from '../terminal/useTerminalState';
import {
  connectBrowserTransport,
  type BrowserTransportConnection,
  type FallbackOverrides,
} from '../terminal/connect';
import type { CellState, StyleDefinition, TerminalGridSnapshot, TerminalGridStore } from '../terminal/gridStore';
import { encodeKeyEvent } from '../terminal/keymap';
import type { TerminalTransport } from '../transport/terminalTransport';
import { BackfillController } from '../terminal/backfillController';
import { cn } from '../lib/utils';
import { createConnectionTrace, type ConnectionTrace } from '../lib/connectionTrace';
import { captureTrace, ensureTraceCaptureHelpers, serializeHostFrame } from '../lib/traceCapture';
import type { ServerMessage } from '../transport/signaling';
import type { SecureTransportSummary } from '../transport/webrtc';

function logSizing(step: string, detail: Record<string, unknown>): void {
  if (typeof window === 'undefined') {
    return;
  }
  if (process.env.NODE_ENV === 'production') {
    return;
  }
  try {
    console.info('[beach-terminal][sizing]', step, JSON.stringify(detail));
  } catch (error) {
    console.info('[beach-terminal][sizing]', step, detail, error);
  }
}

function logCellMetric(kind: string, detail: Record<string, unknown>): void {
  if (typeof window === 'undefined') {
    return;
  }
  if (!(window as typeof window & { __BEACH_TRACE?: boolean }).__BEACH_TRACE) {
    return;
  }
  try {
    console.info(`[beach-terminal][${kind}]`, JSON.stringify(detail));
  } catch (error) {
    console.info(`[beach-terminal][${kind}]`, detail, error);
  }
}
import type {
  TerminalSizingStrategy,
  TerminalSizingHostMeta,
  TerminalViewportProposal,
  TerminalScrollPolicy,
} from '../../../private-beach/src/components/terminalSizing';
import { createLegacyTerminalSizingStrategy } from '../../../private-beach/src/components/terminalSizing';

export type TerminalStatus = 'idle' | 'connecting' | 'connected' | 'error' | 'closed';

export type JoinOverlayState =
  | 'idle'
  | 'connecting'
  | 'waiting'
  | 'approved'
  | 'denied'
  | 'disconnected';

const JOIN_WAIT_DEFAULT = 'Waiting for host approval...';
const JOIN_WAIT_INITIAL = 'Connected - waiting for host approval...';
const JOIN_WAIT_HINT_ONE = 'Still waiting... hang tight.';
const JOIN_WAIT_HINT_TWO = 'Still waiting... ask the host to approve.';
const JOIN_APPROVED_MESSAGE = 'Approved - syncing...';
const JOIN_DENIED_MESSAGE = 'Join request was declined by host.';
const JOIN_DISCONNECTED_MESSAGE = 'Disconnected before approval.';
const JOIN_CONNECTING_MESSAGE = 'Connecting to host...';
const JOIN_OVERLAY_HIDE_DELAY_MS = 1500;
const FALLBACK_ENTITLEMENT_SUBSTRING =
  'WebSocket fallback is only available to Beach Auth subscribers';
const FALLBACK_SIGNUP_MESSAGE =
  "We couldn't negotiate a peer-to-peer connection. WebSocket fallback is reserved for Beach Auth subscribers - visit https://beach.sh to sign up and unlock fallback support.";

declare global {
  interface Window {
    __BEACH_TRACE?: boolean;
    __BEACH_TRACE_DUMP_ROWS?: (limit?: number) => void;
    __BEACH_TRACE_LAST_ROWS?: unknown;
  }
}

function isDevEnvironment(): boolean {
  if (typeof import.meta !== 'undefined') {
    const metaEnv = (import.meta as Record<string, any>).env;
    if (metaEnv && typeof metaEnv.DEV === 'boolean') {
      return Boolean(metaEnv.DEV);
    }
  }
  const nodeEnv = (globalThis as Record<string, any>).process?.env?.NODE_ENV;
  if (typeof nodeEnv === 'string') {
    return nodeEnv !== 'production';
  }
  return false;
}

const IS_DEV = isDevEnvironment();

let versionLogged = false;

function trace(...args: unknown[]): void {
  if (typeof window !== 'undefined' && window.__BEACH_TRACE) {
    if (!versionLogged) {
      versionLogged = true;
      const version =
        typeof __APP_VERSION__ !== 'undefined' ? __APP_VERSION__ : 'unknown';
      console.info('[beach-surfer] version', version);
    }
    console.debug('[beach-trace][terminal]', ...args);
  }
}

const PREDICTIVE_TRACE_START_MS = now();

function predictiveLog(event: string, fields: Record<string, unknown> = {}): void {
  if (!(typeof window !== 'undefined' && window.__BEACH_TRACE)) {
    return;
  }
  const timestamp = now();
  const payload = {
    source: 'web_client',
    event,
    elapsed_ms: timestamp - PREDICTIVE_TRACE_START_MS,
    ...fields,
  };
  try {
    console.debug('[beach-trace][predictive]', JSON.stringify(payload));
  } catch {
    console.debug('[beach-trace][predictive]', payload);
  }
}

function summarizeSnapshot(store: TerminalGridStore | undefined): void {
  if (!store || !(typeof window !== 'undefined' && window.__BEACH_TRACE)) {
    return;
  }
  const snapshot = store.getSnapshot();
  const preview = snapshot.rows
    .filter((row) => row.kind === 'loaded')
    .slice(0, 5)
    .map((row) => ({
      absolute: row.absolute,
      text: row.kind === 'loaded' ? row.cells.map((cell) => cell.char).join('').trimEnd() : '',
      width: row.kind === 'loaded' ? row.cells.length : 0,
    }));
  trace('snapshot state', {
    baseRow: snapshot.baseRow,
    totalRows: snapshot.rows.length,
    cursor: snapshot.cursorRow !== null && snapshot.cursorCol !== null
      ? `row=${snapshot.cursorRow} col=${snapshot.cursorCol} seq=${snapshot.cursorSeq} visible=${snapshot.cursorVisible}`
      : `null (visible=${snapshot.cursorVisible})`,
    predictedCursor: snapshot.predictedCursor,
    preview,
  });
}

export interface PredictionOverlayState {
  visible: boolean;
  underline: boolean;
}

const DEFAULT_PREDICTION_OVERLAY: PredictionOverlayState = { visible: true, underline: false };

const PREDICTION_HANDSHAKE_OFFSET_THRESHOLD = 16;

const PREDICTION_SRTT_TRIGGER_LOW_MS = 20;
const PREDICTION_SRTT_TRIGGER_HIGH_MS = 30;
const PREDICTION_FLAG_TRIGGER_LOW_MS = 50;
const PREDICTION_FLAG_TRIGGER_HIGH_MS = 80;
const PREDICTION_GLITCH_THRESHOLD_MS = 250;
const PREDICTION_GLITCH_REPAIR_COUNT = 10;
const PREDICTION_GLITCH_REPAIR_MIN_INTERVAL_MS = 150;
const PREDICTION_GLITCH_FLAG_THRESHOLD_MS = 5000;
const PREDICTION_SRTT_ALPHA = 0.125;
const PREDICTION_ACK_GRACE_MS = 90;

// Guardrail: implicit host PTY resizes are extremely dangerous while the viewport
// measurement stack is under active refactor. Keep this false unless a user
// explicitly opts into host resizing via dedicated controls.
const ENABLE_IMPLICIT_HOST_RESIZE = false;

type JoinStateSnapshot = { state: JoinOverlayState; message: string | null };

type JoinStateListener = {
  id: string;
  callback: (snapshot: JoinStateSnapshot) => void;
};

const LAST_JOIN_STATE = new Map<string, JoinStateSnapshot>();
const JOIN_STATE_LISTENERS = new Map<string, Map<string, JoinStateListener>>();
let joinStateSubscriberCounter = 0;

function nextJoinStateSubscriberId(): string {
  joinStateSubscriberCounter += 1;
  return `terminal-${joinStateSubscriberCounter}`;
}

function usePredictionUx(
  enabled: boolean,
  onOverlayUpdate: (state: PredictionOverlayState) => void,
): {
  tick: (timestamp: number) => PredictionOverlayState | null;
  recordSend: (seq: number, timestampMs: number, predicted: boolean) => PredictionOverlayState | null;
  recordAck: (seq: number, timestamp: number) => PredictionOverlayState | null;
  reset: (timestamp: number) => PredictionOverlayState | null;
} {
  const uxRef = useRef<PredictionUx | null>(enabled ? new PredictionUx() : null);

  useEffect(() => {
    if (!enabled) {
      uxRef.current = null;
      onOverlayUpdate(DEFAULT_PREDICTION_OVERLAY);
      return;
    }
    if (!uxRef.current) {
      uxRef.current = new PredictionUx();
    }
  }, [enabled, onOverlayUpdate]);

  const tick = useCallback(
    (timestamp: number) => {
      if (!enabled || !uxRef.current) {
        return null;
      }
      const update = uxRef.current.tick(timestamp);
      if (update) {
        onOverlayUpdate(update);
      }
      return update ?? null;
    },
    [enabled, onOverlayUpdate],
  );

  const recordSend = useCallback(
    (seq: number, timestampMs: number, predicted: boolean) => {
      if (!enabled || !uxRef.current) {
        return null;
      }
      const update = uxRef.current.recordSend(seq, timestampMs, predicted);
      if (update) {
        onOverlayUpdate(update);
      }
      return update ?? null;
    },
    [enabled, onOverlayUpdate],
  );

  const recordAck = useCallback(
    (seq: number, timestamp: number) => {
      if (!enabled || !uxRef.current) {
        return null;
      }
      const update = uxRef.current.recordAck(seq, timestamp);
      if (update) {
        onOverlayUpdate(update);
      }
      return update ?? null;
    },
    [enabled, onOverlayUpdate],
  );

  const reset = useCallback(
    (timestamp: number) => {
      if (!uxRef.current) {
        return null;
      }
      const update = uxRef.current.reset(timestamp);
      if (update) {
        onOverlayUpdate(update);
      }
      return update ?? null;
    },
    [onOverlayUpdate],
  );

  return { tick, recordSend, recordAck, reset };
}

function subscribeJoinState(
  sessionId: string,
  listener: JoinStateListener,
): () => void {
  let listeners = JOIN_STATE_LISTENERS.get(sessionId);
  if (!listeners) {
    listeners = new Map();
    JOIN_STATE_LISTENERS.set(sessionId, listeners);
  }
  listeners.set(listener.id, listener);
  return () => {
    const current = JOIN_STATE_LISTENERS.get(sessionId);
    if (!current) return;
    current.delete(listener.id);
    if (current.size === 0) {
      JOIN_STATE_LISTENERS.delete(sessionId);
    }
  };
}

function emitJoinState(
  sessionId: string,
  snapshot: JoinStateSnapshot,
  skipListenerId?: string,
): void {
  const listeners = JOIN_STATE_LISTENERS.get(sessionId);
  if (!listeners) {
    return;
  }
  listeners.forEach(({ id, callback }) => {
    if (skipListenerId && id === skipListenerId) {
      return;
    }
    try {
      callback(snapshot);
    } catch (error) {
      if (IS_DEV) {
        console.warn('[beach-surfer] join state listener error', error);
      }
    }
  });
}

function now(): number {
  if (typeof performance !== 'undefined' && typeof performance.now === 'function') {
    return performance.now();
  }
  return Date.now();
}

function hasPredictiveByte(payload: Uint8Array): boolean {
  for (const value of payload) {
    if (value === 0x0a || value === 0x0d) {
      continue;
    }
    if (value >= 0x20 && value !== 0x7f) {
      return true;
    }
  }
  return false;
}

class PredictionUx {
  private srttMs: number | null = null;
  private srttTrigger = false;
  private flagging = false;
  private glitchTrigger = 0;
  private lastQuickConfirmation = 0;
  private pending = new Map<number, number>();
  private overlay: PredictionOverlayState = { visible: false, underline: false };

  private log(event: string, fields: Record<string, unknown> = {}): void {
    predictiveLog(event, {
      component: 'PredictionUx',
      pending: this.pending.size,
      srtt_ms: this.srttMs,
      glitch_trigger: this.glitchTrigger,
      ...fields,
    });
  }

  recordSend(seq: number, timestampMs: number, predicted: boolean): PredictionOverlayState | null {
    if (!predicted) {
      this.log('prediction_send', { seq, predicted: false, timestamp_ms: timestampMs });
      return null;
    }
    this.pending.set(seq, timestampMs);
    this.log('prediction_send', { seq, predicted: true, timestamp_ms: timestampMs });
    return this.updateOverlayState();
  }

  recordAck(seq: number, timestampMs: number): PredictionOverlayState | null {
    const sentAt = this.pending.get(seq);
    let delayMs: number | null = null;
    if (sentAt !== undefined) {
      this.pending.delete(seq);
      const sample = Math.max(0, timestampMs - sentAt);
      delayMs = sample;
      this.srttMs = this.srttMs === null ? sample : this.srttMs + (sample - this.srttMs) * PREDICTION_SRTT_ALPHA;
      if (this.glitchTrigger > 0 && sample < PREDICTION_GLITCH_THRESHOLD_MS) {
        if (timestampMs - this.lastQuickConfirmation >= PREDICTION_GLITCH_REPAIR_MIN_INTERVAL_MS) {
          this.glitchTrigger = Math.max(0, this.glitchTrigger - 1);
          this.lastQuickConfirmation = timestampMs;
        }
      }
    }
    const overlay = this.updateOverlayState();
    this.log('prediction_ack', { seq, ack_delay_ms: delayMs, cleared: sentAt !== undefined });
    return overlay;
  }

  tick(timestampMs: number): PredictionOverlayState | null {
    let glitch = this.glitchTrigger;
    for (const sentAt of this.pending.values()) {
      const age = timestampMs - sentAt;
      if (age >= PREDICTION_GLITCH_FLAG_THRESHOLD_MS) {
        glitch = Math.max(glitch, PREDICTION_GLITCH_REPAIR_COUNT * 2);
        break;
      }
      if (age >= PREDICTION_GLITCH_THRESHOLD_MS) {
        glitch = Math.max(glitch, PREDICTION_GLITCH_REPAIR_COUNT);
      }
    }
    if (glitch !== this.glitchTrigger) {
      this.glitchTrigger = glitch;
    }
    return this.updateOverlayState();
  }

  reset(timestampMs: number): PredictionOverlayState | null {
    this.srttMs = null;
    this.srttTrigger = false;
    this.flagging = false;
    this.glitchTrigger = 0;
    this.lastQuickConfirmation = 0;
    this.pending.clear();
    const overlay = this.updateOverlayState();
    this.log('prediction_state_reset');
    return overlay;
  }

  private updateOverlayState(): PredictionOverlayState | null {
    const srtt = this.srttMs ?? 0;

    if (srtt > PREDICTION_FLAG_TRIGGER_HIGH_MS || this.glitchTrigger > PREDICTION_GLITCH_REPAIR_COUNT) {
      this.flagging = true;
    } else if (
      this.flagging &&
      srtt <= PREDICTION_FLAG_TRIGGER_LOW_MS &&
      this.glitchTrigger <= PREDICTION_GLITCH_REPAIR_COUNT
    ) {
      this.flagging = false;
    }

    if (srtt > PREDICTION_SRTT_TRIGGER_HIGH_MS || this.glitchTrigger > 0) {
      this.srttTrigger = true;
    } else if (this.srttTrigger && srtt <= PREDICTION_SRTT_TRIGGER_LOW_MS && this.pending.size === 0) {
      this.srttTrigger = false;
    }

    const hasPending = this.pending.size > 0;

    const visible = hasPending || this.srttTrigger || this.glitchTrigger > 0;
    const underline =
      visible && (this.flagging || this.glitchTrigger > PREDICTION_GLITCH_REPAIR_COUNT);

    if (visible === this.overlay.visible && underline === this.overlay.underline) {
      return null;
    }

    this.overlay = { visible, underline };
    this.log('overlay_state', { visible, underline, srtt_trigger: this.srttTrigger });
    return this.overlay;
  }
}

export interface BeachTerminalProps {
  sessionId?: string;
  baseUrl?: string;
  passcode?: string;
  autoConnect?: boolean;
  onStatusChange?: (status: TerminalStatus) => void;
  store?: TerminalGridStore;
  transport?: TerminalTransport;
  transportVersion?: number;
  className?: string;
  fontFamily?: string;
  fontSize?: number;
  showStatusBar?: boolean;
  isFullscreen?: boolean;
  onToggleFullscreen?: (next: boolean) => void;
  showTopBar?: boolean;
  fallbackOverrides?: FallbackOverrides;
  // Danger: implicit PTY resizing can clobber host layouts. Leave false unless explicitly requested.
  autoResizeHostOnViewportChange?: boolean;
  onViewportStateChange?: (state: TerminalViewportState) => void;
  disableViewportMeasurements?: boolean;
  forcedViewportRows?: number | null;
  hideIdlePlaceholder?: boolean;
  // Maximum render FPS for internal rAF-based updates; undefined or <=0 disables throttling
  maxRenderFps?: number;
  viewOnly?: boolean;
  sizingStrategy?: TerminalSizingStrategy;
  enablePredictiveEcho?: boolean;
  enableKeyboardShortcuts?: boolean;
  showJoinOverlay?: boolean;
  onJoinStateChange?: (snapshot: { state: JoinOverlayState; message: string | null }) => void;
}

export type FollowTailPhase = 'hydrating' | 'follow_tail' | 'manual_scrollback' | 'catching_up';

export interface TerminalViewportState {
  viewportRows: number;
  viewportCols: number;
  hostViewportRows: number | null;
  hostCols: number | null;
  canSendResize: boolean;
  viewOnly: boolean;
  sendHostResize?: () => void;
  requestHostResize?: (opts: { rows: number; cols?: number }) => void;
  followTailDesired: boolean;
  followTailPhase: FollowTailPhase;
  atTail: boolean;
  remainingTailPixels: number;
  tailPaddingRows: number;
  pixelsPerRow: number | null;
  pixelsPerCol: number | null;
}

export function BeachTerminal(props: BeachTerminalProps): JSX.Element {
  const MAX_VIEWPORT_ROWS = 512;
  const MAX_VIEWPORT_COLS = 512;
  const MIN_STABLE_VIEWPORT_ROWS = 6;
  const {
    sessionId,
    baseUrl,
    passcode,
    autoConnect = false,
    onStatusChange,
    transport: providedTransport,
    transportVersion = 0,
    store: providedStore,
    fallbackOverrides,
    className,
    fontFamily = "'SFMono-Regular', 'Menlo', 'Consolas', monospace",
    fontSize = 14,
    showStatusBar = true,
    isFullscreen = false,
    onToggleFullscreen,
    showTopBar = true,
    autoResizeHostOnViewportChange = false,
    onViewportStateChange,
    disableViewportMeasurements = false,
    forcedViewportRows = null,
    hideIdlePlaceholder = false,
    maxRenderFps,
    viewOnly = false,
    sizingStrategy: providedSizingStrategy,
    enablePredictiveEcho = true,
    enableKeyboardShortcuts = true,
    showJoinOverlay = true,
    onJoinStateChange,
  } = props;

  const store = useMemo(() => providedStore ?? createTerminalStore(), [providedStore]);
  const sizingStrategy = useMemo<TerminalSizingStrategy>(
    () => providedSizingStrategy ?? createLegacyTerminalSizingStrategy(),
    [providedSizingStrategy],
  );
  const scrollPolicy = useMemo<TerminalScrollPolicy>(
    () => sizingStrategy.scrollPolicy(),
    [sizingStrategy],
  );
  const autoResizeHostOnViewportChangeEffective =
    ENABLE_IMPLICIT_HOST_RESIZE && !viewOnly && autoResizeHostOnViewportChange;
  if (IS_DEV && typeof window !== 'undefined') {
    (window as any).beachStore = store;
  }
  const snapshot = useTerminalSnapshot(store);
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const containerStyleKeysRef = useRef<string[]>([]);
  const headerRef = useRef<HTMLDivElement | null>(null);
  const transportRef = useRef<TerminalTransport | null>(providedTransport ?? null);
  const connectionRef = useRef<BrowserTransportConnection | null>(null);
  const connectionTraceRef = useRef<ConnectionTrace | null>(null);
  const subscriptionRef = useRef<number | null>(null);
  const inputSeqRef = useRef(0);
  // Micro-batching input to reduce per-key overhead and make paste fast.
  const pendingInputRef = useRef<Array<{ data: Uint8Array; predict: boolean }>>([]);
  const flushTimerRef = useRef<number | null>(null);
  const lastSentViewportRows = useRef<number>(0);
  const lastMeasuredViewportRows = useRef<number>(24);
  const suppressNextResizeRef = useRef<boolean>(false);
  const sendHostResizeRef = useRef<() => void>(() => {});
  const lastViewportReportRef = useRef<{
    viewportRows: number;
    viewportCols: number;
    hostViewportRows: number | null;
    hostCols: number | null;
    canSendResize: boolean;
    viewOnly: boolean;
    followTailDesired: boolean;
    followTailPhase: FollowTailPhase;
    atTail: boolean;
    remainingTailPixels: number;
    tailPaddingRows: number;
    pixelsPerRow: number | null;
    pixelsPerCol: number | null;
  } | null>(null);
  const disableMeasurementsPrevRef = useRef<boolean>(disableViewportMeasurements);
  const pixelsPerRowRef = useRef<number | null>(null);
  const pixelsPerColRef = useRef<number | null>(null);
  const [status, setStatus] = useState<TerminalStatus>(
    providedTransport ? 'connected' : 'idle',
  );
  const [error, setError] = useState<Error | null>(null);
  const [secureSummary, setSecureSummary] = useState<SecureTransportSummary | null>(null);
  const [showIdlePlaceholder, setShowIdlePlaceholder] = useState(!hideIdlePlaceholder);
  const [, setHeaderHeight] = useState<number>(0);
  const [activeConnection, setActiveConnection] = useState<BrowserTransportConnection | null>(null);
  const [peerId, setPeerId] = useState<string | null>(null);
  const [remotePeerId, setRemotePeerId] = useState<string | null>(null);
  const [joinState, setJoinState] = useState<JoinOverlayState>('idle');
  const joinStateRef = useRef<JoinOverlayState>('idle');
  const [joinMessage, setJoinMessage] = useState<string | null>(null);
  const joinMessageRef = useRef<string | null>(null);
  const [predictionOverlay, setPredictionOverlay] = useState<PredictionOverlayState>({
    visible: false,
    underline: false,
  });
  const initialFollowTailDesired = useMemo(() => {
    const snapshotNow = store.getSnapshot();
    if (snapshotNow.rows.length > 0) {
      return snapshotNow.followTail;
    }
    return true;
  }, [store]);
  const [followTailDesiredState, setFollowTailDesiredState] = useState<boolean>(initialFollowTailDesired);
  const followTailDesiredRef = useRef(followTailDesiredState);
  const [followTailPhaseState, setFollowTailPhaseState] = useState<FollowTailPhase>('hydrating');
  const followTailPhaseRef = useRef<FollowTailPhase>('hydrating');
  const hydratingRef = useRef<boolean>(true);
  const programmaticScrollRef = useRef<boolean>(false);
  const lastScrollSnapshotRef = useRef<{ top: number; height: number }>({ top: 0, height: 0 });
  const tailMetricsRef = useRef<{ remainingPixels: number; atTail: boolean; paddingRows: number }>({
    remainingPixels: Number.POSITIVE_INFINITY,
    atTail: false,
    paddingRows: 0,
  });
  const tailPaddingRowsRef = useRef<number>(0);
  const {
    tick: predictionTick,
    recordSend: recordPredictionSend,
    recordAck: recordPredictionAck,
    reset: resetPrediction,
  } = usePredictionUx(enablePredictiveEcho, setPredictionOverlay);
  const [ptyViewportRows, setPtyViewportRows] = useState<number | null>(null);
  const ptyViewportRowsRef = useRef<number | null>(null);
  const [ptyCols, setPtyCols] = useState<number | null>(null);
  const ptyColsRef = useRef<number | null>(null);
  const [subscriptionVersion, setSubscriptionVersion] = useState(0);
  // Debounce DOM-driven viewport measurements to avoid transient oscillation
  // from StrictMode remounts and layout thrash from rewriting the PTY size.
  const domPendingViewportRowsRef = useRef<number | null>(null);
  const domPendingTimestampRef = useRef<number | null>(null);
  const domPendingTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [measuredCellWidth, setMeasuredCellWidth] = useState<number | null>(null);
  const clearDomPendingTimeout = useCallback(() => {
    if (domPendingTimeoutRef.current !== null) {
      clearTimeout(domPendingTimeoutRef.current);
      domPendingTimeoutRef.current = null;
    }
  }, []);
  const DOM_VIEWPORT_DEBOUNCE_MS = 120;
  const DOM_VIEWPORT_TOLERANCE = 1; // rows
  const devicePixelRatioValue =
    typeof window !== 'undefined' && typeof window.devicePixelRatio === 'number'
      ? window.devicePixelRatio || 1
      : 1;
  const baseCellWidth = (fontSize / BASE_TERMINAL_FONT_SIZE) * BASE_TERMINAL_CELL_WIDTH;
  const dpr = Math.max(1, devicePixelRatioValue);
  const roundedCellWidth = Math.max(1, Math.round(baseCellWidth * dpr) / dpr);
  const cellWidthPx = Number(roundedCellWidth.toFixed(3));
  const effectiveCellWidth = measuredCellWidth && measuredCellWidth > 0 ? measuredCellWidth : cellWidthPx;
  const lastSentViewportCols = useRef<number | null>(null);
  const computeViewportGeometry = useCallback(
    (rect: DOMRectReadOnly) => {
      const container = containerRef.current;
      let availableWidth =
        container && typeof container.clientWidth === 'number' ? container.clientWidth : rect.width;
      let paddingLeft = 0;
      let paddingRight = 0;
      if (container && typeof window !== 'undefined') {
        const computed = window.getComputedStyle(container);
        const parsedLeft = Number.parseFloat(computed.paddingLeft ?? '0');
        const parsedRight = Number.parseFloat(computed.paddingRight ?? '0');
        paddingLeft = Number.isFinite(parsedLeft) ? parsedLeft : 0;
        paddingRight = Number.isFinite(parsedRight) ? parsedRight : 0;
      }
      if (!Number.isFinite(availableWidth) || availableWidth <= 0) {
        availableWidth = Number.isFinite(rect.width) ? rect.width : 0;
      }
      const innerWidth = Math.max(0, availableWidth - paddingLeft - paddingRight);
      let measuredCols: number | null = null;
      if (innerWidth > 0 && effectiveCellWidth > 0) {
        measuredCols = Math.max(1, Math.floor(innerWidth / effectiveCellWidth));
      }
      const snapshotNow = store.getSnapshot();
      const hostCols =
        ptyColsRef.current && ptyColsRef.current > 0
          ? ptyColsRef.current
          : snapshotNow.cols && snapshotNow.cols > 0
            ? snapshotNow.cols
            : DEFAULT_TERMINAL_COLS;
      const measuredTarget =
        measuredCols != null ? Math.max(measuredCols, hostCols) : hostCols;
      const targetCols = Math.max(1, Math.min(measuredTarget, MAX_VIEWPORT_COLS));
      return { innerWidth, measuredCols, targetCols, snapshotNow };
    },
    [DEFAULT_TERMINAL_COLS, MAX_VIEWPORT_COLS, effectiveCellWidth, store],
  );
  const effectiveOverlay = useMemo(() => {
    if (!enablePredictiveEcho) {
      return DEFAULT_PREDICTION_OVERLAY;
    }
    if (predictionOverlay.visible || !snapshot.hasPredictions) {
      return predictionOverlay;
    }
    return { ...predictionOverlay, visible: true };
  }, [enablePredictiveEcho, predictionOverlay, snapshot.hasPredictions]);
  const joinTimersRef = useRef<{ short?: number; long?: number; hide?: number }>({});
  const peerIdRef = useRef<string | null>(null);
  const handshakeReadyRef = useRef(false);
  const markConnectionTrace = useCallback(
    (name: string, extra: Record<string, unknown> = {}) => {
      connectionTraceRef.current?.mark(name, extra);
    },
    [],
  );

  const setFollowTailDesired = useCallback(
    (desired: boolean, reason: string) => {
      if (followTailDesiredRef.current === desired) {
        return;
      }
      followTailDesiredRef.current = desired;
      setFollowTailDesiredState(desired);
      trace('follow_tail_intent', { desired, reason });
    },
    [],
  );

  const applyFollowTailIntent = useCallback(
    (reason: string) => {
      const desired = followTailDesiredRef.current;
      const phase = followTailPhaseRef.current;
      const hydrating = hydratingRef.current;
      trace('follow_tail_apply_intent', { desired, hydrating, phase, reason });
      const effective = hydrating ? false : desired && phase !== 'manual_scrollback';
      store.setFollowTail(effective);
    },
    [store],
  );

  const setFollowTailPhase = useCallback(
    (phase: FollowTailPhase, reason: string) => {
      if (followTailPhaseRef.current === phase) {
        return;
      }
      followTailPhaseRef.current = phase;
      setFollowTailPhaseState(phase);
      trace('follow_tail_phase', { phase, reason });
    },
    [],
  );

  const enterManualScrollback = useCallback(
    (reason: string) => {
      setFollowTailDesired(false, reason);
      setFollowTailPhase('manual_scrollback', reason);
      applyFollowTailIntent(reason);
    },
    [applyFollowTailIntent, setFollowTailDesired, setFollowTailPhase],
  );

  const enterTailIntent = useCallback(
    (reason: string) => {
      setFollowTailDesired(true, reason);
      const nextPhase = tailPaddingRowsRef.current > 0 ? 'catching_up' : 'follow_tail';
      setFollowTailPhase(nextPhase, reason);
      applyFollowTailIntent(reason);
    },
    [applyFollowTailIntent, setFollowTailDesired, setFollowTailPhase],
  );

  const updateTailMetrics = useCallback(
    (remainingPixels: number, atTail: boolean, reason: string) => {
      const state = tailMetricsRef.current;
      if (state.remainingPixels === remainingPixels && state.atTail === atTail) {
        return;
      }
      state.remainingPixels = remainingPixels;
      state.atTail = atTail;
      trace('follow_tail_metrics', { remainingPixels, atTail, paddingRows: state.paddingRows, reason });
    },
    [],
  );

  const syncTailPadding = useCallback(
    (paddingRows: number, reason: string) => {
      const state = tailMetricsRef.current;
      if (state.paddingRows === paddingRows) {
        return;
      }
      state.paddingRows = paddingRows;
      trace('follow_tail_metrics', {
        remainingPixels: state.remainingPixels,
        atTail: state.atTail,
        paddingRows,
        reason,
      });
    },
    [],
  );

  const subscriberIdRef = useRef<string>('');
  if (!subscriberIdRef.current) {
    subscriberIdRef.current = nextJoinStateSubscriberId();
  }

  const updateJoinStateCache = useCallback(
    (state: JoinOverlayState, message: string | null, skipEmitId?: string) => {
      if (!sessionId) {
        return;
      }
      const previous = LAST_JOIN_STATE.get(sessionId);
      if (previous && previous.state === state && previous.message === message) {
        return;
      }
      const snapshot = { state, message } as JoinStateSnapshot;
      if (typeof window !== 'undefined') {
        try {
          console.info(
            '[terminal][diag] join-state-cache',
            JSON.stringify({ sessionId, state, message }),
          );
        } catch {
          // ignore logging failures
        }
      }
      LAST_JOIN_STATE.set(sessionId, snapshot);
      emitJoinState(sessionId, snapshot, skipEmitId);
      if (onJoinStateChange) {
        onJoinStateChange(snapshot);
      }
    },
    [onJoinStateChange, sessionId],
  );
  const finishConnectionTrace = useCallback(
    (outcome: 'success' | 'error' | 'cancelled', extra: Record<string, unknown> = {}) => {
      if (!connectionTraceRef.current) {
        return;
      }
      connectionTraceRef.current.finish(outcome, extra);
      connectionTraceRef.current = null;
    },
    [],
  );
  const queryLabel = useMemo(() => {
    if (typeof window === 'undefined') {
      return undefined;
    }
    const label = new URLSearchParams(window.location.search).get('label');
    if (!label) {
      return undefined;
    }
    const trimmed = label.trim();
    return trimmed.length > 0 ? trimmed : undefined;
  }, []);
  const clearJoinTimers = useCallback(() => {
    const timers = joinTimersRef.current;
    if (timers.short !== undefined) {
      window.clearTimeout(timers.short);
      timers.short = undefined;
    }
    if (timers.long !== undefined) {
      window.clearTimeout(timers.long);
      timers.long = undefined;
    }
    if (timers.hide !== undefined) {
      window.clearTimeout(timers.hide);
      timers.hide = undefined;
    }
  }, []);
  useEffect(() => {
    peerIdRef.current = peerId;
  }, [peerId]);
  useEffect(() => {
    joinStateRef.current = joinState;
  }, [joinState]);
  useEffect(() => {
    joinMessageRef.current = joinMessage;
  }, [joinMessage]);
  useEffect(() => {
    return () => {
      clearJoinTimers();
    };
  }, [clearJoinTimers]);
  useEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }
    let raf = 0;
    const intervalMs = (() => {
      if (typeof maxRenderFps !== 'number' || maxRenderFps <= 0) return 0;
      const ms = 1000 / Math.max(1, Math.min(120, Math.floor(maxRenderFps)));
      return ms;
    })();
    let lastTick = 0;
    const step = () => {
      const timestamp = now();
      if (!intervalMs || timestamp - lastTick >= intervalMs) {
        lastTick = timestamp;
        store.pruneAckedPredictions(timestamp, PREDICTION_ACK_GRACE_MS);
        if (enablePredictiveEcho) {
          predictionTick(timestamp);
        }
      }
      raf = window.requestAnimationFrame(step);
    };
    raf = window.requestAnimationFrame(step);
    return () => {
      if (raf) {
        window.cancelAnimationFrame(raf);
      }
    };
  }, [store, maxRenderFps, enablePredictiveEcho, predictionTick]);
  const log = useCallback((message: string, detail?: Record<string, unknown>) => {
    if (typeof window === 'undefined' || !window.__BEACH_TRACE) {
      return;
    }
    const current = peerIdRef.current;
    const prefix = current ? `[beach-surfer:${current.slice(0, 8)}]` : '[beach-surfer]';
    if (detail) {
      console.info(`${prefix} ${message}`, detail);
    } else {
      console.info(`${prefix} ${message}`);
    }
  }, []);
  const applyContainerSizing = useCallback((style: CSSProperties | undefined) => {
    const container = containerRef.current;
    if (!container) {
      return;
    }

    if (containerStyleKeysRef.current.length > 0) {
      for (const key of containerStyleKeysRef.current) {
        if (key.startsWith('--')) {
          container.style.removeProperty(key);
        } else {
          (container.style as any)[key] = '';
        }
      }
      containerStyleKeysRef.current = [];
    }

    if (!style) {
      return;
    }

    const nextKeys: string[] = [];
    Object.entries(style).forEach(([key, value]) => {
      if (value == null) {
        return;
      }
      nextKeys.push(key);
      if (key.startsWith('--')) {
        container.style.setProperty(key, String(value));
      } else {
        const cssValue =
          typeof value === 'number' && Number.isFinite(value) ? `${value}` : String(value);
        (container.style as any)[key] = cssValue;
      }
    });
    containerStyleKeysRef.current = nextKeys;
  }, []);

  const sendHostResize = useCallback(() => {
    if (viewOnly) {
      if (IS_DEV) {
        console.warn('[beach-surfer] sendHostResize called while viewOnly=true; ignoring resize request');
      }
      return;
    }
    const transport = transportRef.current;
    if (!transport) {
      return;
    }
    const measuredRows = Math.max(2, Math.min(lastMeasuredViewportRows.current, MAX_VIEWPORT_ROWS));
    const snapshotNow = store.getSnapshot();
    const fallbackCols = snapshotNow.cols > 0 ? snapshotNow.cols : 80;
    const targetCols = Math.max(1, ptyColsRef.current ?? fallbackCols);
    suppressNextResizeRef.current = true;
    lastSentViewportRows.current = measuredRows;
    const label = queryLabel ?? null;
    const peerId = peerIdRef.current;
    log('send_host_resize', {
      targetCols,
      targetRows: measuredRows,
      clientLabel: label ?? null,
      peerId: peerId ?? null,
      viewOnly: false,
    });
    try {
      transport.send({ type: 'resize', cols: targetCols, rows: measuredRows });
      lastSentViewportCols.current = targetCols;
    } catch (err) {
      if (IS_DEV) {
        console.warn('[beach-surfer] send_host_resize failed', err);
      }
    }
  }, [MAX_VIEWPORT_ROWS, log, store, viewOnly, queryLabel]);
  sendHostResizeRef.current = sendHostResize;

  // Explicit host resize API that bypasses lastMeasuredViewportRows and uses inputs.
  const requestHostResize = useCallback(
    (opts: { rows: number; cols?: number }) => {
      if (viewOnly) {
        if (IS_DEV) {
          console.warn('[beach-surfer] requestHostResize ignored because viewOnly=true');
        }
        return;
      }
      const transport = transportRef.current;
      if (!transport) {
        return;
      }
      const rows = Math.max(2, Math.min(Math.floor(opts.rows), MAX_VIEWPORT_ROWS));
      const snapshotNow = store.getSnapshot();
      const fallbackCols = snapshotNow.cols > 0 ? snapshotNow.cols : 80;
      const cols = Math.max(1, Math.floor(opts.cols ?? ptyColsRef.current ?? fallbackCols));
      suppressNextResizeRef.current = true;
      lastSentViewportRows.current = rows;
      const label = queryLabel ?? null;
      const peerId = peerIdRef.current;
      log('request_host_resize', {
        targetCols: cols,
        targetRows: rows,
        reason: 'explicit',
        clientLabel: label ?? null,
        peerId: peerId ?? null,
        viewOnly: false,
      });
      try {
        transport.send({ type: 'resize', cols, rows });
        lastSentViewportCols.current = cols;
      } catch (err) {
        if (IS_DEV) {
          console.warn('[beach-surfer] request_host_resize failed', err);
        }
      }
    },
    [MAX_VIEWPORT_ROWS, log, store, viewOnly, queryLabel],
  );

  const emitViewportState = useCallback(() => {
    if (!onViewportStateChange) {
      return;
    }
    const snapshotNow = store.getSnapshot();
    const viewportRows = Math.max(1, Math.min(lastMeasuredViewportRows.current, MAX_VIEWPORT_ROWS));
    const viewportCols = snapshotNow.cols;
    const hostViewportRows = ptyViewportRowsRef.current;
    const hostCols = ptyColsRef.current;
    const canSendResize = Boolean(transportRef.current) && !viewOnly;
    tailMetricsRef.current.paddingRows = tailPaddingRowsRef.current;
    const tailMetrics = tailMetricsRef.current;
    const followTailDesired = followTailDesiredRef.current;
    const followTailPhase = followTailPhaseRef.current;
    const pixelsPerRow = pixelsPerRowRef.current;
    const pixelsPerCol = pixelsPerColRef.current;
    const nextReport = {
      viewportRows,
      viewportCols,
      hostViewportRows,
      hostCols,
      canSendResize,
      viewOnly,
      followTailDesired,
      followTailPhase,
      atTail: tailMetrics.atTail,
      remainingTailPixels: tailMetrics.remainingPixels,
      tailPaddingRows: tailMetrics.paddingRows,
      pixelsPerRow,
      pixelsPerCol,
    };
    const previous = lastViewportReportRef.current;
    trace(
      'viewport state candidate',
      JSON.stringify({
        viewportRows,
        viewportCols,
        hostViewportRows,
        hostCols,
        canSendResize,
        viewOnly,
        followTailDesired,
        followTailPhase,
        atTail: tailMetrics.atTail,
        remainingTailPixels: tailMetrics.remainingPixels,
        tailPaddingRows: tailMetrics.paddingRows,
        pixelsPerRow,
        pixelsPerCol,
        suppressed: Boolean(
          previous &&
            previous.viewportRows === nextReport.viewportRows &&
            previous.viewportCols === nextReport.viewportCols &&
            previous.hostViewportRows === nextReport.hostViewportRows &&
            previous.hostCols === nextReport.hostCols &&
            previous.canSendResize === nextReport.canSendResize &&
            previous.viewOnly === nextReport.viewOnly &&
            previous.followTailDesired === nextReport.followTailDesired &&
            previous.followTailPhase === nextReport.followTailPhase &&
            previous.atTail === nextReport.atTail &&
            previous.remainingTailPixels === nextReport.remainingTailPixels &&
            previous.tailPaddingRows === nextReport.tailPaddingRows &&
            previous.pixelsPerRow === nextReport.pixelsPerRow &&
            previous.pixelsPerCol === nextReport.pixelsPerCol,
        ),
      }),
    );
    if (
      previous &&
      previous.viewportRows === nextReport.viewportRows &&
      previous.viewportCols === nextReport.viewportCols &&
        previous.hostViewportRows === nextReport.hostViewportRows &&
        previous.hostCols === nextReport.hostCols &&
        previous.canSendResize === nextReport.canSendResize &&
        previous.viewOnly === nextReport.viewOnly &&
        previous.followTailDesired === nextReport.followTailDesired &&
        previous.followTailPhase === nextReport.followTailPhase &&
        previous.atTail === nextReport.atTail &&
        previous.remainingTailPixels === nextReport.remainingTailPixels &&
        previous.tailPaddingRows === nextReport.tailPaddingRows &&
        previous.pixelsPerRow === nextReport.pixelsPerRow &&
        previous.pixelsPerCol === nextReport.pixelsPerCol
    ) {
      return;
    }
    lastViewportReportRef.current = nextReport;
    if (typeof window !== 'undefined') {
      try {
        console.info('[terminal][trace] emit-viewport-state', {
          sessionId,
          viewportRows: nextReport.viewportRows,
          viewportCols: nextReport.viewportCols,
          hostViewportRows: nextReport.hostViewportRows,
          hostCols: nextReport.hostCols,
          canSendResize,
          viewOnly: nextReport.viewOnly,
          followTailDesired: nextReport.followTailDesired,
          followTailPhase: nextReport.followTailPhase,
          atTail: nextReport.atTail,
          remainingTailPixels: nextReport.remainingTailPixels,
          tailPaddingRows: nextReport.tailPaddingRows,
          pixelsPerRow: nextReport.pixelsPerRow,
          pixelsPerCol: nextReport.pixelsPerCol,
        });
      } catch {
        // ignore logging issues
      }
    }
    const payload: TerminalViewportState = {
      ...nextReport,
    };
    if (!viewOnly) {
      payload.sendHostResize = sendHostResizeRef.current;
      payload.requestHostResize = requestHostResize;
    }
    onViewportStateChange(payload);
  }, [MAX_VIEWPORT_ROWS, onViewportStateChange, requestHostResize, store, viewOnly]);

  useEffect(() => {
    lastViewportReportRef.current = null;
    emitViewportState();
  }, [emitViewportState]);

  useEffect(() => {
    const normalized =
      Number.isFinite(effectiveCellWidth) && effectiveCellWidth > 0 ? Number(effectiveCellWidth) : null;
    if (pixelsPerColRef.current !== normalized) {
      pixelsPerColRef.current = normalized;
      emitViewportState();
    }
  }, [effectiveCellWidth, emitViewportState]);

  const exitHydration = useCallback(
    (reason: string) => {
      if (!hydratingRef.current) {
        return;
      }
      hydratingRef.current = false;
      if (followTailDesiredRef.current) {
        const phase = tailPaddingRowsRef.current > 0 ? 'catching_up' : 'follow_tail';
        setFollowTailPhase(phase, reason);
      } else {
        setFollowTailPhase('manual_scrollback', reason);
      }
      applyFollowTailIntent(reason);
      emitViewportState();
    },
    [applyFollowTailIntent, emitViewportState, setFollowTailPhase],
  );

  useEffect(() => {
    followTailDesiredRef.current = followTailDesiredState;
    applyFollowTailIntent('intent-state-change');
  }, [followTailDesiredState, applyFollowTailIntent]);

  useEffect(() => {
    followTailPhaseRef.current = followTailPhaseState;
    applyFollowTailIntent('phase-state-change');
  }, [followTailPhaseState, applyFollowTailIntent]);

  const enterWaitingState = useCallback(
    (message?: string) => {
      handshakeReadyRef.current = false;
      const trimmed = message?.trim();
      const effective = trimmed && trimmed.length > 0 ? trimmed : JOIN_WAIT_INITIAL;
      setJoinState('waiting');
      setJoinMessage(effective);
      updateJoinStateCache('waiting', effective ?? null, subscriberIdRef.current);
      clearJoinTimers();
      if (!trimmed || trimmed.length === 0) {
        joinTimersRef.current.short = window.setTimeout(() => {
          setJoinMessage((current) => {
            if (!current) {
              return current;
            }
            if (current === JOIN_WAIT_INITIAL || current === JOIN_WAIT_DEFAULT) {
              updateJoinStateCache('waiting', JOIN_WAIT_HINT_ONE, subscriberIdRef.current);
              return JOIN_WAIT_HINT_ONE;
            }
            return current;
          });
        }, 10_000);
        joinTimersRef.current.long = window.setTimeout(() => {
          setJoinMessage((current) => {
            if (!current) {
              return current;
            }
            if (
              current === JOIN_WAIT_INITIAL ||
              current === JOIN_WAIT_DEFAULT ||
              current === JOIN_WAIT_HINT_ONE
            ) {
              updateJoinStateCache('waiting', JOIN_WAIT_HINT_TWO, subscriberIdRef.current);
              return JOIN_WAIT_HINT_TWO;
            }
            return current;
          });
        }, 30_000);
      }
    },
    [clearJoinTimers, updateJoinStateCache],
  );

  const enterApprovedState = useCallback(
    (message?: string) => {
      handshakeReadyRef.current = true;
      const trimmed = message?.trim();
      const effective = trimmed && trimmed.length > 0 ? trimmed : JOIN_APPROVED_MESSAGE;
      setJoinState('approved');
      setJoinMessage(effective);
      updateJoinStateCache('approved', effective ?? null, subscriberIdRef.current);
      clearJoinTimers();
      joinTimersRef.current.hide = window.setTimeout(() => {
        setJoinState('idle');
        setJoinMessage(null);
        updateJoinStateCache('idle', null, subscriberIdRef.current);
        joinTimersRef.current.hide = undefined;
      }, JOIN_OVERLAY_HIDE_DELAY_MS);
    },
    [clearJoinTimers, updateJoinStateCache],
  );

  const enterDeniedState = useCallback(
    (message?: string) => {
      handshakeReadyRef.current = false;
      const trimmed = message?.trim();
      const effective = trimmed && trimmed.length > 0 ? trimmed : JOIN_DENIED_MESSAGE;
      setJoinState('denied');
      setJoinMessage(effective);
      updateJoinStateCache('denied', effective ?? null, subscriberIdRef.current);
      clearJoinTimers();
      joinTimersRef.current.hide = window.setTimeout(() => {
        setJoinState('idle');
        setJoinMessage(null);
        updateJoinStateCache('idle', null, subscriberIdRef.current);
        joinTimersRef.current.hide = undefined;
      }, JOIN_OVERLAY_HIDE_DELAY_MS);
    },
    [clearJoinTimers, updateJoinStateCache],
  );

  const enterDisconnectedState = useCallback(() => {
    handshakeReadyRef.current = false;
    setJoinState('disconnected');
    setJoinMessage(JOIN_DISCONNECTED_MESSAGE);
    updateJoinStateCache('disconnected', JOIN_DISCONNECTED_MESSAGE, subscriberIdRef.current);
    clearJoinTimers();
    joinTimersRef.current.hide = window.setTimeout(() => {
      setJoinState('idle');
      setJoinMessage(null);
      updateJoinStateCache('idle', null, subscriberIdRef.current);
      joinTimersRef.current.hide = undefined;
    }, JOIN_OVERLAY_HIDE_DELAY_MS);
  }, [clearJoinTimers, updateJoinStateCache]);

  const handleStatusSignal = useCallback(
    (signal: string) => {
      if (!signal.startsWith('beach:status:')) {
        return;
      }
      const payload = signal.slice('beach:status:'.length);
      const [kind, ...rest] = payload.split(' ');
      const detail = rest.join(' ').trim();
      switch (kind) {
        case 'approval_pending':
          if (handshakeReadyRef.current) {
            return;
          }
          enterWaitingState(detail.length > 0 ? detail : undefined);
          break;
        case 'approval_granted':
          enterApprovedState(detail.length > 0 ? detail : undefined);
          break;
        case 'approval_denied':
          enterDeniedState(detail.length > 0 ? detail : undefined);
          break;
        default:
          break;
      }
    },
    [enterApprovedState, enterDeniedState, enterWaitingState],
  );
  useEffect(() => {
    if (!sessionId) {
      return;
    }
    const applySnapshot = (snapshot: JoinStateSnapshot) => {
      const { state, message } = snapshot;
      if (typeof window !== 'undefined') {
        try {
          console.info('[terminal][diag] join-state-apply', {
            sessionId,
            state,
            message,
            currentState: joinStateRef.current,
            currentMessage: joinMessageRef.current,
          });
        } catch {
          // ignore logging failures
        }
      }
      if (state === joinStateRef.current && message === joinMessageRef.current) {
        return;
      }
      switch (state) {
        case 'waiting':
          enterWaitingState(message ?? undefined);
          break;
        case 'approved':
          enterApprovedState(message ?? undefined);
          break;
        case 'denied':
          enterDeniedState(message ?? undefined);
          break;
        case 'disconnected':
          enterDisconnectedState();
          break;
        case 'connecting':
          setJoinState('connecting');
          setJoinMessage(message ?? JOIN_CONNECTING_MESSAGE);
          break;
        case 'idle':
          setJoinState('idle');
          setJoinMessage(message ?? null);
          break;
        default:
          break;
      }
      if (onJoinStateChange) {
        onJoinStateChange({ state, message });
      }
    };
    const cached = LAST_JOIN_STATE.get(sessionId);
    if (cached) {
      applySnapshot(cached);
    }
    return subscribeJoinState(sessionId, {
      id: subscriberIdRef.current,
      callback: applySnapshot,
    });
  }, [
    enterApprovedState,
    enterDeniedState,
    enterDisconnectedState,
    enterWaitingState,
    joinMessage,
    joinState,
    onJoinStateChange,
    sessionId,
    updateJoinStateCache,
  ]);
  useEffect(() => {
    if (hideIdlePlaceholder) {
      setShowIdlePlaceholder(false);
      onStatusChange?.(status);
      return;
    }
    onStatusChange?.(status);
    if (status === 'connected') {
      setShowIdlePlaceholder(false);
      // Auto-focus terminal when connected
      if (containerRef.current) {
        containerRef.current.focus();
      }
    } else if (status === 'idle') {
      setShowIdlePlaceholder(true);
    }
  }, [hideIdlePlaceholder, status, onStatusChange]);
  const lines = useMemo(() => buildLines(snapshot, Number.POSITIVE_INFINITY, effectiveOverlay), [snapshot, effectiveOverlay]);
  const placeholderRowsInViewport = useMemo(
    () => lines.reduce((count, line) => (line.kind === 'loaded' ? count : count + 1), 0),
    [lines],
  );

  useEffect(() => {
    tailPaddingRowsRef.current = placeholderRowsInViewport;
    syncTailPadding(placeholderRowsInViewport, 'placeholder-sync');
    if (
      !hydratingRef.current &&
      followTailDesiredRef.current &&
      followTailPhaseRef.current !== 'manual_scrollback'
    ) {
      const nextPhase = placeholderRowsInViewport > 0 ? 'catching_up' : 'follow_tail';
      if (followTailPhaseRef.current !== nextPhase) {
        setFollowTailPhase(nextPhase, 'placeholder-sync');
        applyFollowTailIntent('placeholder-sync');
      }
    }
  }, [
    placeholderRowsInViewport,
    applyFollowTailIntent,
    setFollowTailPhase,
    syncTailPadding,
  ]);
  if (IS_DEV && typeof window !== 'undefined' && window.__BEACH_TRACE) {
    const sample = lines.slice(-5).map((line) => ({
      absolute: line.absolute,
      kind: line.kind,
      text: line.cells?.map((cell) => (cell.char === '\u00a0' ? ' ' : cell.char)).join('') ?? null,
    }));
    console.debug('[beach-trace][terminal] render sample', { count: lines.length, sample });
  }
  const sessionTitle = useMemo(() => {
    if (sessionId && sessionId.trim().length > 0) {
      const trimmed = sessionId.trim();
      return trimmed.length > 24 ? `${trimmed.slice(0, 12)}…${trimmed.slice(-6)}` : trimmed;
    }
    return 'New Session';
  }, [sessionId]);
  if (IS_DEV && typeof window !== 'undefined') {
    (window as any).beachLines = lines;
  }
  const lineHeight = computeLineHeight(fontSize);
  const [measuredLineHeight, setMeasuredLineHeight] = useState<number>(lineHeight);
  const minEffectiveLineHeight = Math.max(4, lineHeight * 0.4);
  const effectiveLineHeight = measuredLineHeight >= minEffectiveLineHeight ? measuredLineHeight : lineHeight;
  const applyFontMetrics = useCallback(() => {
    const metrics = measureFontGlyphMetrics(fontFamily, fontSize);
    if (!metrics) {
      return false;
    }
    const { cellWidth, lineHeight: measuredHeight } = metrics;
    setMeasuredCellWidth((prev) => {
      if (prev === null || Math.abs(prev - cellWidth) > 0.05) {
        return cellWidth;
      }
      return prev;
    });
    setMeasuredLineHeight((prev) => {
      if (Math.abs(prev - measuredHeight) > 0.1) {
        return measuredHeight;
      }
      return prev;
    });
    return true;
  }, [fontFamily, fontSize]);
  useEffect(() => {
    const normalized =
      Number.isFinite(effectiveLineHeight) && effectiveLineHeight > 0 ? Number(effectiveLineHeight) : null;
    if (pixelsPerRowRef.current !== normalized) {
      pixelsPerRowRef.current = normalized;
      emitViewportState();
    }
  }, [effectiveLineHeight, emitViewportState]);
  const totalRows = snapshot.rows.length;
  const firstAbsolute = lines.length > 0 ? lines[0]!.absolute : snapshot.baseRow;
  const lastAbsolute = lines.length > 0 ? lines[lines.length - 1]!.absolute : firstAbsolute;
  const lastContentAbsolute = findLastContentAbsolute(snapshot);
  const topPaddingRows = Math.max(0, firstAbsolute - snapshot.baseRow);
  const minimumViewportRows = snapshot.viewportHeight > 0 ? snapshot.viewportHeight : Math.max(lines.length, 1);
  const contentRows = lastContentAbsolute !== null ? Math.max(0, lastContentAbsolute - snapshot.baseRow + 1) : 0;
  const effectiveTotalRows = Math.max(minimumViewportRows, contentRows);
  const bottomPaddingRows = Math.max(0, snapshot.baseRow + effectiveTotalRows - (lastAbsolute + 1));
  const topPadding = topPaddingRows * effectiveLineHeight;
  const bottomPadding = bottomPaddingRows * effectiveLineHeight;
  const backfillController = useMemo(
    () => new BackfillController(store, (frame) => transportRef.current?.send(frame)),
    [store],
  );

  useLayoutEffect(() => {
    if (!showTopBar) {
      setHeaderHeight(0);
      return;
    }
    if (typeof window === 'undefined') {
      return;
    }
    const header = headerRef.current;
    if (!header) {
      return;
    }
    let raf = -1;
    const measure = () => {
      const rect = header.getBoundingClientRect();
      const next = rect.height;
      if (!Number.isFinite(next) || next <= 0) {
        return;
      }
      setHeaderHeight((prev) => (Math.abs(prev - next) > 0.5 ? next : prev));
    };
    measure();
    if ('ResizeObserver' in window) {
      const observer = new ResizeObserver(() => {
        if (raf !== -1) {
          window.cancelAnimationFrame(raf);
        }
        raf = window.requestAnimationFrame(measure);
      });
      observer.observe(header);
      return () => {
        observer.disconnect();
        if (raf !== -1) {
          window.cancelAnimationFrame(raf);
        }
      };
    }
    const win: Window & typeof globalThis = window;
    const handleResize = () => {
      if (raf !== -1) {
        win.cancelAnimationFrame(raf);
      }
      raf = win.requestAnimationFrame(measure);
    };
    win.addEventListener('resize', handleResize);
    return () => {
      win.removeEventListener('resize', handleResize);
      if (raf !== -1) {
        win.cancelAnimationFrame(raf);
      }
    };
  }, [showTopBar]);

  useLayoutEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }
    // Measure actual glyph metrics so CSS cell sizing and viewport math reflect the true font/zoom.
    const container = containerRef.current;
    if (!container) {
      return;
    }
    applyFontMetrics();
    let frame = -1;
    const scheduleMeasure = () => {
      if (frame !== -1) {
        window.cancelAnimationFrame(frame);
      }
      frame = window.requestAnimationFrame(() => {
        applyFontMetrics();
        const row = container.querySelector<HTMLDivElement>('.xterm-row');
        if (!row) {
          return;
        }
        const rect = row.getBoundingClientRect();
        if (Number.isFinite(rect.height) && rect.height > 0) {
          const minAcceptableLineHeight = Math.max(lineHeight * 0.9, minEffectiveLineHeight);
          if (typeof window !== 'undefined' && window.__BEACH_TRACE) {
            try {
              const containerSample = containerRef.current;
              const rowClassList = Array.from(row.classList ?? []);
              const dataset = row.dataset ? { ...row.dataset } : undefined;
              const textSample = row.textContent
                ? row.textContent.replace(/\u00a0/g, ' ').slice(0, 64)
                : null;
              const sample = {
                height: Number(rect.height.toFixed(3)),
                classList: rowClassList,
                dataset,
                childCount: row.childElementCount,
                scrollTop: containerSample?.scrollTop ?? null,
                clientHeight: containerSample?.clientHeight ?? null,
                scrollHeight: containerSample?.scrollHeight ?? null,
                textSample,
                clamped: rect.height < minAcceptableLineHeight,
              };
              console.info('[beach-terminal][measure] row-height-sample', JSON.stringify(sample));
            } catch (error) {
              console.warn('[beach-terminal][measure] row-height-log-failed', error);
            }
          }
          setMeasuredLineHeight((prev) => {
            const next = rect.height;
            if (next >= minAcceptableLineHeight) {
              return Math.abs(prev - next) > 0.1 ? next : prev;
            }
            if (prev < minEffectiveLineHeight) {
              return lineHeight;
            }
            return prev;
          });
        }
        let nextCellWidth: number | null = null;
        let measurementSource: 'glyph_span' | 'row_width' | 'fallback' = 'fallback';
        const spans = Array.from(row.querySelectorAll<HTMLSpanElement>('span'));
        const glyphSpan = spans.find((span) => {
          const text = span.textContent?.replace(/\u00A0/g, ' ').trim() ?? '';
          return text.length > 0;
        }) ?? spans[0] ?? null;
        if (glyphSpan) {
          const glyphRect = glyphSpan.getBoundingClientRect();
          if (Number.isFinite(glyphRect.width) && glyphRect.width > 0) {
            nextCellWidth = glyphRect.width;
            measurementSource = 'glyph_span';
          }
        }
        if ((!nextCellWidth || nextCellWidth <= 0) && Number.isFinite(rect.width) && rect.width > 0) {
          const renderedCells = spans.length;
          const fallbackCols = snapshot.cols > 0 ? snapshot.cols : DEFAULT_TERMINAL_COLS;
          const colsCount = renderedCells > 0 ? renderedCells : fallbackCols;
          const widthPerCell = rect.width / Math.max(1, colsCount);
          if (Number.isFinite(widthPerCell) && widthPerCell > 0) {
            nextCellWidth = widthPerCell;
            measurementSource = 'row_width';
          }
        }
        if (nextCellWidth && nextCellWidth > 0) {
          logCellMetric('dom-measure', {
            sessionId: sessionId ?? null,
            source: measurementSource,
            nextCellWidth,
            spans: spans.length,
            snapshotCols: snapshot.cols,
            baseCellWidth,
            fontFamily,
            fontSize,
            rowHeight: rect.height,
          });
          setMeasuredCellWidth((prev) =>
            prev === null || Math.abs(prev - nextCellWidth) > 0.1 ? nextCellWidth : prev,
          );
        }
      });
    };

    scheduleMeasure();
    let observer: ResizeObserver | null = null;
    if (typeof ResizeObserver !== 'undefined') {
      observer = new ResizeObserver(() => scheduleMeasure());
      observer.observe(container);
    }
    window.addEventListener('resize', scheduleMeasure);
    const vv = window.visualViewport;
    if (vv) {
      vv.addEventListener('resize', scheduleMeasure);
      vv.addEventListener('scroll', scheduleMeasure);
    }
    return () => {
      if (frame !== -1) {
        window.cancelAnimationFrame(frame);
      }
      if (observer) {
        observer.disconnect();
      }
      window.removeEventListener('resize', scheduleMeasure);
      if (vv) {
        vv.removeEventListener('resize', scheduleMeasure);
        vv.removeEventListener('scroll', scheduleMeasure);
      }
    };
  }, [snapshot.cols, fontFamily, fontSize, lines.length, lineHeight, minEffectiveLineHeight, applyFontMetrics]);
  useEffect(() => {
    transportRef.current = providedTransport ?? null;
    lastSentViewportRows.current = 0;
    lastSentViewportCols.current = null;
    setSecureSummary(null);
    ptyViewportRowsRef.current = null;
    setPtyViewportRows((prev) => (prev === null ? prev : null));
    ptyColsRef.current = null;
    setPtyCols((prev) => (prev === null ? prev : null));
    if (providedTransport) {
      bindTransport(providedTransport);
      setStatus('connected');
      if (!handshakeReadyRef.current) {
        enterWaitingState();
      }
    }
    if (!providedTransport) {
      handshakeReadyRef.current = false;
      clearJoinTimers();
      setJoinState('idle');
      setJoinMessage(null);
      updateJoinStateCache('idle', null, subscriberIdRef.current);
    }
    emitViewportState();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [providedTransport, transportVersion]);

  useEffect(() => {
    if (!autoConnect || transportRef.current || !sessionId || !baseUrl) {
      return;
    }
    finishConnectionTrace('cancelled', { reason: 'restart' });
    connectionTraceRef.current = createConnectionTrace({ sessionId, baseUrl });
    markConnectionTrace('beach_terminal:connect_initiated', {
      autoConnect: true,
      hasPasscode: Boolean(passcode),
    });
    let cancelled = false;
    setStatus('connecting');
    setJoinState('connecting');
    setJoinMessage(JOIN_CONNECTING_MESSAGE);
    updateJoinStateCache('connecting', JOIN_CONNECTING_MESSAGE, subscriberIdRef.current);
    markConnectionTrace('beach_terminal:status_connecting');
    handshakeReadyRef.current = false;
    clearJoinTimers();
    (async () => {
      try {
        const webrtcLogger = (message: string) => {
          const noisyPrefixes = [
            'sending local candidate',
            'local candidate queued',
            'received remote ice candidate',
            'ice add ok',
          ];
          if (noisyPrefixes.some((prefix) => message.startsWith(prefix))) {
            return;
          }
          log(message);
        };

        markConnectionTrace('beach_terminal:transport_connect_start', { baseUrl, sessionId });
        const connection = await connectBrowserTransport({
          sessionId,
          baseUrl,
          passcode,
          logger: webrtcLogger,
          clientLabel: queryLabel,
          fallbackOverrides,
          trace: connectionTraceRef.current,
        });
        markConnectionTrace('beach_terminal:transport_connect_success', {
          remotePeerId: connection.remotePeerId ?? null,
        });
        if (cancelled) {
          markConnectionTrace('beach_terminal:transport_connect_cancelled');
          connection.close();
          return;
        }
        connectionRef.current = connection;
        transportRef.current = connection.transport;
        setActiveConnection(connection);
        setSecureSummary(connection.secure ?? null);
        setPeerId(connection.signaling.peerId);
        setRemotePeerId(connection.remotePeerId ?? null);
        bindTransport(connection.transport);
        setStatus('connected');
        emitViewportState();
        markConnectionTrace('beach_terminal:status_connected');
      } catch (err) {
        if (cancelled) {
          markConnectionTrace('beach_terminal:transport_connect_cancelled_error');
          return;
        }
        const errorInstance = err instanceof Error ? err : new Error(String(err));
        console.error('[beach-surfer] transport connect failed', errorInstance);
        const message = errorInstance.message ?? '';
        const fallbackDenied = message.includes(FALLBACK_ENTITLEMENT_SUBSTRING);
        setJoinState('denied');
        const displayMessage = fallbackDenied
          ? FALLBACK_SIGNUP_MESSAGE
          : message || 'Unable to connect to the host.';
        setJoinMessage(displayMessage);
        setError(errorInstance);
        setStatus('error');
        markConnectionTrace('beach_terminal:transport_connect_error', {
          message: errorInstance.message,
        });
        finishConnectionTrace('error', { stage: 'connect', message: errorInstance.message });
      }
    })();

    return () => {
      cancelled = true;
      markConnectionTrace('beach_terminal:connect_effect_cleanup');
      if (connectionRef.current) {
        connectionRef.current.close();
      }
      connectionRef.current = null;
      transportRef.current = null;
      lastSentViewportRows.current = 0;
      lastSentViewportCols.current = null;
      setActiveConnection(null);
      setSecureSummary(null);
      setPeerId(null);
      setRemotePeerId(null);
      handshakeReadyRef.current = false;
      clearJoinTimers();
      setJoinState('idle');
      setJoinMessage(null);
      emitViewportState();
      finishConnectionTrace('cancelled', { reason: 'cleanup' });
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoConnect, sessionId, baseUrl, passcode, queryLabel]);

  const buildHostMeta = useCallback(
    (rect: DOMRectReadOnly): TerminalSizingHostMeta => {
      let preferred: number | null = null;
      if (typeof forcedViewportRows === 'number' && forcedViewportRows > 0) {
        preferred = forcedViewportRows;
      } else if (ptyViewportRowsRef.current && ptyViewportRowsRef.current > 0) {
        preferred = ptyViewportRowsRef.current;
      } else if (
        typeof globalThis !== 'undefined' &&
        (globalThis as Record<string, any>).ENABLE_VISIBLE_PREVIEW_DRIVER
      ) {
        try {
          const snapshotNow = store.getSnapshot();
          const candidate = Math.max(
            snapshotNow.viewportHeight > 0 ? snapshotNow.viewportHeight : 0,
            snapshotNow.rows.length,
          );
          if (candidate > 0) {
            preferred = candidate;
          }
        } catch {
          // ignore snapshot errors
        }
      }
      const meta: TerminalSizingHostMeta = {
        lineHeightPx: effectiveLineHeight,
        minViewportRows: MIN_STABLE_VIEWPORT_ROWS,
        maxViewportRows: MAX_VIEWPORT_ROWS,
        lastViewportRows: lastMeasuredViewportRows.current,
        disableViewportMeasurements,
        forcedViewportRows: typeof forcedViewportRows === 'number' ? forcedViewportRows : null,
        preferredViewportRows: preferred,
        windowInnerHeightPx:
          typeof window !== 'undefined' && typeof window.innerHeight === 'number'
            ? window.innerHeight
            : null,
        defaultViewportRows: 24,
      };
      logSizing('buildHostMeta', {
        sessionId: sessionId ?? null,
        rectHeight: Number.isFinite(rect.height) ? Number(rect.height.toFixed(2)) : rect.height,
        disableViewportMeasurements,
        preferredViewportRows: preferred,
        forcedViewportRows: typeof forcedViewportRows === 'number' ? forcedViewportRows : null,
        lastViewportRows: lastMeasuredViewportRows.current,
      });
      return meta;
    },
    [disableViewportMeasurements, effectiveLineHeight, forcedViewportRows, store],
  );

  const proposeViewport = useCallback(
    (source: string, rect: DOMRectReadOnly, meta: TerminalSizingHostMeta): TerminalViewportProposal => {
      const proposal = sizingStrategy.nextViewport(rect, meta);
      logSizing('nextViewport', {
        source,
        rectHeight: Number.isFinite(rect.height) ? Number(rect.height.toFixed(2)) : rect.height,
        rectWidth: Number.isFinite(rect.width) ? Number(rect.width.toFixed(2)) : rect.width,
        viewportRows: proposal.viewportRows ?? null,
        measuredRows: proposal.measuredRows ?? null,
        fallbackRows: proposal.fallbackRows ?? null,
        minViewportRows: meta.minViewportRows,
        maxViewportRows: meta.maxViewportRows,
        preferredViewportRows: meta.preferredViewportRows ?? null,
        lastViewportRows: meta.lastViewportRows ?? null,
        forcedViewportRows: meta.forcedViewportRows ?? null,
        disableViewportMeasurements: meta.disableViewportMeasurements,
      });
      return proposal;
    },
    [sizingStrategy],
  );

  const commitViewportRows = useCallback(
    (
      rows: number,
      rect: DOMRectReadOnly,
      hostMeta: TerminalSizingHostMeta,
      proposal?: TerminalViewportProposal,
      options?: { forceLastSent?: boolean },
    ) => {
      const container = containerRef.current;
      if (!container) {
        return;
      }
      const clampedRows = Math.max(1, Math.min(Math.round(rows), hostMeta.maxViewportRows));
      const { innerWidth, measuredCols, targetCols, snapshotNow: current } = computeViewportGeometry(rect);
      const previousSentCols = lastSentViewportCols.current;
      log('resize-commit', {
        containerHeight: rect.height,
        viewportHeight: rect.height,
        lineHeight,
        measuredRows: proposal?.measuredRows,
        fallbackRows: proposal?.fallbackRows,
        committedRows: clampedRows,
        lastSent: lastSentViewportRows.current,
        baseRow: current.baseRow,
        totalRows: current.rows.length,
        followTail: current.followTail,
        measuredCols,
        targetCols,
        previousSentCols,
        snapshotCols: (current as any).cols ?? null,
        hostCols: ptyColsRef.current,
      });
      logSizing('commitViewportRows', {
        rows,
        clampedRows,
        measuredRows: proposal?.measuredRows ?? null,
        fallbackRows: proposal?.fallbackRows ?? null,
        forceLastSent: Boolean(options?.forceLastSent),
        lastSentViewportRows: lastSentViewportRows.current,
        viewportTop: current.viewportTop,
        totalRows: current.rows.length,
        measuredCols,
        innerWidth,
        targetCols,
        cellWidth: effectiveCellWidth,
        lastSentViewportCols: previousSentCols ?? null,
      });
      store.setViewport(current.viewportTop, clampedRows);
      lastMeasuredViewportRows.current = clampedRows;
      emitViewportState();
      const snapshotAfterCommit = store.getSnapshot();
      backfillController.maybeRequest(snapshotAfterCommit, {
        nearBottom: snapshotAfterCommit.followTail,
        followTailDesired: followTailDesiredRef.current,
        phase: followTailPhaseRef.current,
        tailPaddingRows: tailPaddingRowsRef.current,
      });
      if (suppressNextResizeRef.current) {
        suppressNextResizeRef.current = false;
      } else if (
        autoResizeHostOnViewportChangeEffective &&
        subscriptionRef.current !== null &&
        transportRef.current &&
        (clampedRows !== lastSentViewportRows.current ||
          targetCols !== previousSentCols)
      ) {
        const label = queryLabel ?? null;
        const peerId = peerIdRef.current;
        log('auto_host_resize', {
          targetCols,
          targetRows: clampedRows,
          clientLabel: label ?? null,
          peerId: peerId ?? null,
          viewOnly,
        });
        transportRef.current.send({ type: 'resize', cols: targetCols, rows: clampedRows });
        lastSentViewportRows.current = clampedRows;
        lastSentViewportCols.current = targetCols;
      }
      if (options?.forceLastSent) {
        lastSentViewportRows.current = clampedRows;
        lastSentViewportCols.current = targetCols;
      }
      const style = sizingStrategy.containerStyle(rect, hostMeta, clampedRows);
      applyContainerSizing(style);
    },
    [
      backfillController,
      applyContainerSizing,
      autoResizeHostOnViewportChangeEffective,
      computeViewportGeometry,
      emitViewportState,
      lineHeight,
      log,
      queryLabel,
      sizingStrategy,
      viewOnly,
    ],
  );

  const scheduleViewportCommit = useCallback(
    (proposal: TerminalViewportProposal, rect: DOMRectReadOnly, hostMeta: TerminalSizingHostMeta) => {
      if (!proposal || proposal.viewportRows == null) {
        return;
      }
      if (hostMeta.disableViewportMeasurements) {
        const fallbackCandidate =
          proposal.viewportRows ??
          hostMeta.lastViewportRows ??
          hostMeta.preferredViewportRows ??
          hostMeta.defaultViewportRows;
        const numericFallback =
          typeof fallbackCandidate === 'number' && Number.isFinite(fallbackCandidate)
            ? Math.max(1, Math.round(fallbackCandidate))
            : hostMeta.defaultViewportRows;
        const clampedFallback = Math.max(
          hostMeta.minViewportRows,
          Math.min(numericFallback, hostMeta.maxViewportRows),
        );
        const style = sizingStrategy.containerStyle(rect, hostMeta, clampedFallback);
        applyContainerSizing(style);
        logSizing('scheduleViewportCommit:skip-disabled', {
          fallbackCandidate,
          clampedFallback,
        });
        return;
      }
      const candidateRows = Math.max(1, Math.round(proposal.viewportRows));
      const previousViewportRows = lastMeasuredViewportRows.current;
      const nowTs = now();
      const pending = domPendingViewportRowsRef.current;
      logSizing('scheduleViewportCommit', {
        candidateRows,
        measuredRows: proposal.measuredRows ?? null,
        fallbackRows: proposal.fallbackRows ?? null,
        previousViewportRows: previousViewportRows ?? null,
        pendingRows: pending ?? null,
        pendingAgeMs:
          domPendingTimestampRef.current != null ? nowTs - domPendingTimestampRef.current : null,
      });
      if (
        previousViewportRows != null &&
        Math.abs(previousViewportRows - candidateRows) <= DOM_VIEWPORT_TOLERANCE
      ) {
        domPendingViewportRowsRef.current = null;
        domPendingTimestampRef.current = null;
        clearDomPendingTimeout();
        const styleRows = previousViewportRows ?? candidateRows;
        const geometry = computeViewportGeometry(rect);
        const previousSentCols = lastSentViewportCols.current;
        const shouldForceCommit =
          subscriptionRef.current !== null &&
          (geometry.targetCols !== previousSentCols || previousSentCols === null);
        if (shouldForceCommit) {
          commitViewportRows(styleRows, rect, hostMeta, proposal);
        } else {
          const style = sizingStrategy.containerStyle(rect, hostMeta, styleRows);
          applyContainerSizing(style);
        }
        logSizing('scheduleViewportCommit:reuse-previous', {
          styleRows,
          candidateRows,
          domTolerance: DOM_VIEWPORT_TOLERANCE,
          targetCols: geometry.targetCols,
          previousSentCols,
        });
        return;
      }
      if (
        pending != null &&
        Math.abs(pending - candidateRows) <= DOM_VIEWPORT_TOLERANCE &&
        domPendingTimestampRef.current != null &&
        nowTs - domPendingTimestampRef.current >= DOM_VIEWPORT_DEBOUNCE_MS
      ) {
        domPendingViewportRowsRef.current = null;
        domPendingTimestampRef.current = null;
        clearDomPendingTimeout();
        commitViewportRows(candidateRows, rect, hostMeta, proposal);
        logSizing('scheduleViewportCommit:commit-pending', {
          candidateRows,
          debounceMs: DOM_VIEWPORT_DEBOUNCE_MS,
        });
        return;
      }
      const pendingAge =
        domPendingTimestampRef.current != null ? nowTs - domPendingTimestampRef.current : null;
      if (
        pending != null &&
        pendingAge != null &&
        pendingAge >= DOM_VIEWPORT_DEBOUNCE_MS * 2 &&
        Math.abs(pending - candidateRows) <= DOM_VIEWPORT_TOLERANCE
      ) {
        const stale = pending;
        domPendingViewportRowsRef.current = null;
        domPendingTimestampRef.current = null;
        clearDomPendingTimeout();
        commitViewportRows(stale, rect, hostMeta, proposal);
        logSizing('scheduleViewportCommit:commit-stale', {
          staleRows: stale,
          pendingAge,
        });
        return;
      }
      if (pending == null || Math.abs(pending - candidateRows) > DOM_VIEWPORT_TOLERANCE) {
        if (
          pending != null &&
          Math.abs(pending - candidateRows) > DOM_VIEWPORT_TOLERANCE &&
          domPendingTimestampRef.current != null
        ) {
          logSizing('scheduleViewportCommit:replace-pending', {
            previousPendingRows: pending,
            candidateRows,
            pendingAge,
          });
        }
        domPendingViewportRowsRef.current = candidateRows;
        domPendingTimestampRef.current = nowTs;
        clearDomPendingTimeout();
        log('resize-sample', {
          measuredRows: proposal.measuredRows,
          candidateRows,
          fallbackRows: proposal.fallbackRows,
          previousViewportRows,
        });
        logSizing('scheduleViewportCommit:set-pending', {
          candidateRows,
          previousViewportRows: previousViewportRows ?? null,
        });
        if (typeof window !== 'undefined') {
          domPendingTimeoutRef.current = window.setTimeout(() => {
            if (
              domPendingViewportRowsRef.current === candidateRows &&
              domPendingTimestampRef.current != null
            ) {
              const age = now() - domPendingTimestampRef.current;
              domPendingViewportRowsRef.current = null;
              domPendingTimestampRef.current = null;
              clearDomPendingTimeout();
              commitViewportRows(candidateRows, rect, hostMeta, proposal);
              logSizing('scheduleViewportCommit:timeout-commit', {
                candidateRows,
                pendingAgeMs: age,
              });
            }
          }, DOM_VIEWPORT_DEBOUNCE_MS * 2);
        }
      }
    },
    [
      applyContainerSizing,
      clearDomPendingTimeout,
      commitViewportRows,
      computeViewportGeometry,
      log,
      sizingStrategy,
    ],
  );

  const triggerViewportRecalc = useCallback(
    (source: string, force?: boolean) => {
      const wrapper = wrapperRef.current;
      if (!wrapper) {
        return;
      }
      const rect = wrapper.getBoundingClientRect();
      const meta = buildHostMeta(rect);
      const proposal = proposeViewport(source, rect, meta);
      if (force && proposal.viewportRows != null) {
        commitViewportRows(proposal.viewportRows, rect, meta, proposal, { forceLastSent: true });
      } else {
        scheduleViewportCommit(proposal, rect, meta);
      }
    },
    [buildHostMeta, commitViewportRows, proposeViewport, scheduleViewportCommit],
  );

  useEffect(() => {
    const wrapper = wrapperRef.current;
    const container = containerRef.current;
    if (!wrapper || !container) {
      return;
    }

    const initialRect = wrapper.getBoundingClientRect();
    const initialMeta = buildHostMeta(initialRect);

    if (initialMeta.disableViewportMeasurements) {
      const proposal = proposeViewport('initial:disable-measurements', initialRect, initialMeta);
      if (proposal.viewportRows != null) {
        domPendingViewportRowsRef.current = null;
        domPendingTimestampRef.current = null;
        clearDomPendingTimeout();
        commitViewportRows(
          proposal.viewportRows,
          initialRect,
          initialMeta,
          proposal,
          { forceLastSent: true },
        );
      }
      return;
    }

    if (typeof ResizeObserver === 'undefined') {
      return;
    }

    const observer = new ResizeObserver((entries) => {
      const entry = entries[entries.length - 1];
      const rect =
        entry && entry.target instanceof Element
          ? entry.target.getBoundingClientRect()
          : wrapper.getBoundingClientRect();
      const meta = buildHostMeta(rect);
      const proposal = proposeViewport('resize-observer:entry', rect, meta);
      scheduleViewportCommit(proposal, rect, meta);
    });

    observer.observe(wrapper);

    const proposal = proposeViewport('resize-observer:initial', initialRect, initialMeta);
    scheduleViewportCommit(proposal, initialRect, initialMeta);

    return () => {
      observer.disconnect();
      clearDomPendingTimeout();
      domPendingViewportRowsRef.current = null;
      domPendingTimestampRef.current = null;
    };
  }, [
    buildHostMeta,
    commitViewportRows,
    proposeViewport,
    clearDomPendingTimeout,
    scheduleViewportCommit,
    sizingStrategy,
  ]);

  useEffect(() => {
    const previous = disableMeasurementsPrevRef.current;
    if (previous === disableViewportMeasurements) {
      return;
    }
    disableMeasurementsPrevRef.current = disableViewportMeasurements;

    const wrapper = wrapperRef.current;
    if (!wrapper) {
      return;
    }

    const rect = wrapper.getBoundingClientRect();
    const meta = buildHostMeta(rect);

    if (disableViewportMeasurements) {
      const proposal = proposeViewport('toggle:disable-measurements', rect, meta);
      if (proposal.viewportRows != null) {
        domPendingViewportRowsRef.current = null;
        domPendingTimestampRef.current = null;
        clearDomPendingTimeout();
        commitViewportRows(proposal.viewportRows, rect, meta, proposal, { forceLastSent: true });
      }
      return;
    }

    triggerViewportRecalc('toggle:enable-measurements', true);
  }, [
    buildHostMeta,
    clearDomPendingTimeout,
    commitViewportRows,
    disableViewportMeasurements,
    proposeViewport,
    triggerViewportRecalc,
  ]);

  useEffect(() => {
    if (!subscriptionRef.current) {
      return;
    }
    triggerViewportRecalc('subscription-ready', true);
  }, [subscriptionVersion, triggerViewportRecalc]);

  useEffect(() => {
    if (!subscriptionRef.current) {
      return;
    }
    if (measuredCellWidth == null) {
      return;
    }
    triggerViewportRecalc('cell-width-update');
  }, [measuredCellWidth, triggerViewportRecalc]);

  useEffect(() => {
    const connection = activeConnection;
    if (!connection) {
      return;
    }
    const { signaling } = connection;

    const handleMessage = (event: Event) => {
      const detail = (event as CustomEvent<ServerMessage>).detail;
      if (detail.type === 'peer_joined') {
        log('signaling peer_joined', { peerId: detail.peer.id, role: detail.peer.role });
      } else if (detail.type === 'peer_left') {
        log('signaling peer_left', { peerId: detail.peer_id });
      } else if (detail.type === 'error') {
        log('signaling error', { message: detail.message });
      }
    };

    const handleClose = (event: Event) => {
      const detail = (event as CustomEvent<CloseEvent>).detail;
      log('signaling closed', {
        code: detail?.code,
        reason: detail?.reason,
      });
    };

    const handleError = (event: Event) => {
      const err = (event as ErrorEvent).error ?? event;
      log('signaling socket error', {
        message: err instanceof Error ? err.message : String(err),
      });
    };

    signaling.addEventListener('message', handleMessage as EventListener);
    signaling.addEventListener('close', handleClose as EventListener);
    signaling.addEventListener('error', handleError as EventListener);

    return () => {
      signaling.removeEventListener('message', handleMessage as EventListener);
      signaling.removeEventListener('close', handleClose as EventListener);
      signaling.removeEventListener('error', handleError as EventListener);
    };
  }, [activeConnection, log]);

  useEffect(() => {
    const element = containerRef.current;
    if (!element) {
      return;
    }
    lastScrollSnapshotRef.current = { top: element.scrollTop, height: element.clientHeight };
  }, [snapshot.viewportTop, snapshot.viewportHeight]);

  useEffect(() => {
    const element = containerRef.current;
    if (
      !element ||
      scrollPolicy !== 'follow-tail' ||
      !followTailDesiredState ||
      followTailPhaseState === 'manual_scrollback'
    ) {
      return;
    }
    const applyScroll = () => {
      const target = element.scrollHeight - element.clientHeight;
      if (target < 0) {
        return;
      }
      const rowHeight = Math.max(1, effectiveLineHeight);
      const viewportRows = Math.max(1, Math.floor(element.clientHeight / rowHeight));
      let desired = target;
      const lastContentAbsolute = findLastContentAbsolute(snapshot);
      if (lastContentAbsolute !== null && lastContentAbsolute >= snapshot.baseRow) {
        const totalContentRows = lastContentAbsolute - snapshot.baseRow + 1;
        const contentBottom = topPadding + totalContentRows * rowHeight;
        const desiredByContent = Math.max(0, contentBottom - element.clientHeight);
        const scrollableRows = Math.max(0, totalContentRows - viewportRows);
        const contentRowsOffset = topPadding + scrollableRows * rowHeight;
        const boundedDesired = Math.min(desiredByContent, contentRowsOffset);
        desired = Math.min(target, Math.max(0, boundedDesired));
      } else {
        desired = 0;
      }
      if (IS_DEV && typeof window !== 'undefined' && window.__BEACH_TRACE) {
        console.debug('[beach-trace][terminal] autoscroll', {
          before: element.scrollTop,
          target,
          desired,
          scrollHeight: element.scrollHeight,
          clientHeight: element.clientHeight,
          viewportRows,
          totalContentRows: lastContentAbsolute !== null ? lastContentAbsolute - snapshot.baseRow + 1 : 0,
        });
      }
      const currentTop = element.scrollTop;
      const upwardsDelta = currentTop - desired;
      const maxAutoRewind = rowHeight * 1.5;
      // Only adjust upward if we're nudging by about a row; avoid yanking the user to the top.
      if (desired >= currentTop || upwardsDelta <= maxAutoRewind) {
        if (element.scrollTop !== desired) {
          programmaticScrollRef.current = true;
          element.scrollTop = desired;
          if (typeof queueMicrotask === 'function') {
            queueMicrotask(() => {
              programmaticScrollRef.current = false;
            });
          } else {
            setTimeout(() => {
              programmaticScrollRef.current = false;
            }, 0);
          }
        }
      } else if (IS_DEV && typeof window !== 'undefined' && window.__BEACH_TRACE) {
        console.debug('[beach-trace][terminal] autoscroll skipped rewind', {
          before: currentTop,
          desired,
          upwardsDelta,
          maxAutoRewind,
        });
      }
    };
    if (typeof window !== 'undefined' && typeof window.requestAnimationFrame === 'function') {
      let outer = -1;
      let inner = -1;
      outer = window.requestAnimationFrame(() => {
        inner = window.requestAnimationFrame(applyScroll);
      });
      return () => {
        if (outer !== -1) {
          window.cancelAnimationFrame(outer);
        }
        if (inner !== -1) {
          window.cancelAnimationFrame(inner);
        }
      };
    }
    applyScroll();
  }, [
    snapshot.baseRow,
    snapshot.rows.length,
    lastAbsolute,
    lineHeight,
    topPadding,
    bottomPadding,
    lines.length,
    effectiveLineHeight,
    scrollPolicy,
    followTailDesiredState,
    followTailPhaseState,
  ]);

  const INPUT_MAX_FRAME_BYTES = 32 * 1024; // conservative cap per input frame

  const flushPendingInput = useCallback(() => {
    const transport = transportRef.current;
    const subscription = subscriptionRef.current;
    const queue = pendingInputRef.current;
    flushTimerRef.current = null;
    if (!transport || subscription === null || queue.length === 0) {
      pendingInputRef.current = [];
      return;
    }
    // Combine adjacent chunks, then send in size-capped frames.
    let total = 0;
    let allowPredict = true;
    for (const entry of queue) {
      total += entry.data.length;
      if (!entry.predict) allowPredict = false;
    }
    const combined = new Uint8Array(total);
    let offset = 0;
    for (const entry of queue) {
      combined.set(entry.data, offset);
      offset += entry.data.length;
    }
    pendingInputRef.current = [];

    let pos = 0;
    while (pos < combined.length) {
      const end = Math.min(pos + INPUT_MAX_FRAME_BYTES, combined.length);
      const slice = combined.subarray(pos, end);
      pos = end;
      const seq = ++inputSeqRef.current;
      const predicts = allowPredict && hasPredictiveByte(slice) && slice.length <= 32;
      const timestampMs = now();
      transport.send({ type: 'input', seq, data: slice });
      const predictionApplied = predicts ? store.registerPrediction(seq, slice) : false;
      recordPredictionSend(seq, timestampMs, predicts && predictionApplied);
    }
  }, [store, recordPredictionSend]);

  const enqueueInput = useCallback((data: Uint8Array, predict: boolean) => {
    pendingInputRef.current.push({ data, predict });
    if (flushTimerRef.current === null) {
      // Micro-batch on the next task to allow multiple key events in one frame.
      flushTimerRef.current = window.setTimeout(flushPendingInput, 2);
    }
  }, [flushPendingInput]);

  const handleKeyDown: React.KeyboardEventHandler<HTMLDivElement> = (event) => {
    if (!enableKeyboardShortcuts) {
      return;
    }
    const transport = transportRef.current;
    if (!transport) {
      trace('handleKeyDown: no transport');
      return;
    }
    const payload = encodeKeyEvent(event.nativeEvent);
    if (!payload || payload.length === 0) {
      trace('handleKeyDown: no payload', { key: event.key, code: event.code });
      return;
    }
    if (subscriptionRef.current === null) {
      trace('handleKeyDown: no subscription');
      return;
    }
    event.preventDefault();
    enqueueInput(payload, true);
  };

  const handlePaste: React.ClipboardEventHandler<HTMLDivElement> = (event) => {
    if (!enableKeyboardShortcuts) {
      return;
    }
    const transport = transportRef.current;
    if (!transport) {
      trace('handlePaste: no transport');
      return;
    }
    if (subscriptionRef.current === null) {
      trace('handlePaste: no subscription');
      return;
    }
    const text = event.clipboardData?.getData('text') ?? '';
    if (text.length === 0) {
      trace('handlePaste: empty');
      return;
    }
    event.preventDefault();
    const encoder = new TextEncoder();
    const bytes = encoder.encode(text);
    // Treat as a single chunk and skip predictions to reduce CPU during large pastes.
    enqueueInput(bytes, false);
  };

  useEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }
    const dumpRows = (limit?: number) => {
      const snapshot = store.getSnapshot();
      const fallbackHeight = snapshot.viewportHeight || snapshot.rows.length || 1;
      const normalized = typeof limit === 'number' && Number.isFinite(limit) && limit > 0 ? Math.floor(limit) : fallbackHeight;
      const visible = store.visibleRows(normalized);
      const entries = visible.map((row) => {
        if (!row) {
          return null;
        }
        if (row.kind === 'loaded') {
          const text = store.getRowText(row.absolute) ?? '';
          return {
            kind: row.kind,
            absolute: row.absolute,
            seq: row.latestSeq,
            logicalWidth: row.logicalWidth,
            text,
          };
        }
        return {
          kind: row.kind,
          absolute: row.absolute,
          seq: null,
          logicalWidth: null,
          text: '',
        };
      });
      const payload = {
        viewportTop: snapshot.viewportTop,
        viewportHeight: snapshot.viewportHeight,
        followTail: snapshot.followTail,
        baseRow: snapshot.baseRow,
        rowCount: snapshot.rows.length,
        requestedHeight: normalized,
        tailPadSeqThreshold: snapshot.tailPadSeqThreshold,
        tailPadRanges: snapshot.tailPadRanges,
        rows: entries,
      };
      const serialized = JSON.stringify(payload, null, 2);
      console.info('[beach-trace][terminal] dump visible rows\n', serialized);
      window.__BEACH_TRACE_LAST_ROWS = payload;
      const history = (window as typeof window & { __BEACH_TRACE_HISTORY?: unknown[] }).__BEACH_TRACE_HISTORY;
      if (Array.isArray(history)) {
        history.push({
          scope: 'terminal',
          event: 'visibleRows dump',
          payload,
        });
      }
    };
    window.__BEACH_TRACE_DUMP_ROWS = dumpRows;
    return () => {
      if (window.__BEACH_TRACE_DUMP_ROWS === dumpRows) {
        delete window.__BEACH_TRACE_DUMP_ROWS;
      }
    };
  }, [store]);

  useEffect(() => {
    if (!wrapperRef.current) return;

    if (isFullscreen) {
      // Get position before going fullscreen
      const rect = wrapperRef.current.getBoundingClientRect();

      // Start at current position with fixed positioning
      wrapperRef.current.style.position = 'fixed';
      wrapperRef.current.style.top = `${rect.top}px`;
      wrapperRef.current.style.left = `${rect.left}px`;
      wrapperRef.current.style.width = `${rect.width}px`;
      wrapperRef.current.style.height = `${rect.height}px`;
      wrapperRef.current.style.transition = 'none';

      // Force reflow
      wrapperRef.current.offsetHeight;

      // Enable transition and animate to fullscreen
      wrapperRef.current.style.transition = 'all 0.4s cubic-bezier(0.4, 0, 0.2, 1)';

      requestAnimationFrame(() => {
        if (!wrapperRef.current) return;
        wrapperRef.current.style.top = '0';
        wrapperRef.current.style.left = '0';
        wrapperRef.current.style.width = '100vw';
        wrapperRef.current.style.height = '100vh';
      });
    } else {
      // Collapsing from fullscreen
      if (wrapperRef.current.style.position === 'fixed') {
        // Create a temporary placeholder to measure where element should go
        const placeholder = document.createElement('div');
        placeholder.style.position = 'relative';
        placeholder.style.visibility = 'hidden';
        wrapperRef.current.parentElement?.insertBefore(placeholder, wrapperRef.current);

        requestAnimationFrame(() => {
          if (!wrapperRef.current) return;
          const targetRect = placeholder.getBoundingClientRect();
          placeholder.remove();

          // Animate to target position
          wrapperRef.current.style.transition = 'all 0.4s cubic-bezier(0.4, 0, 0.2, 1)';
          wrapperRef.current.style.top = `${targetRect.top}px`;
          wrapperRef.current.style.left = `${targetRect.left}px`;
          wrapperRef.current.style.width = `${targetRect.width}px`;
          wrapperRef.current.style.height = `${targetRect.height}px`;

          // After animation, remove inline styles
          const handler = () => {
            if (!wrapperRef.current) return;
            wrapperRef.current.style.position = '';
            wrapperRef.current.style.top = '';
            wrapperRef.current.style.left = '';
            wrapperRef.current.style.width = '';
            wrapperRef.current.style.height = '';
            wrapperRef.current.style.transition = '';
            wrapperRef.current.removeEventListener('transitionend', handler);
          };
          wrapperRef.current.addEventListener('transitionend', handler);
        });
      }
    }
  }, [isFullscreen]);

  const wrapperClasses = cn(
    'relative flex h-full min-h-0 flex-col',
    'rounded-[22px] border border-[#0f131a] bg-[#090d14]/95 shadow-[0_45px_120px_-70px_rgba(10,26,55,0.85)]',
    isFullscreen && 'z-50 rounded-none',
    className,
  );
  const headerClasses = cn(
    'relative z-10 flex items-center justify-between gap-4 bg-[#111925]/95 px-6 py-3 text-[11px] font-medium uppercase tracking-[0.36em] text-[#9aa4bc]',
    !isFullscreen && 'rounded-t-[22px]',
  );
  const containerClasses = cn(
    'beach-terminal relative flex-1 min-h-0 overflow-y-auto overflow-x-auto whitespace-pre font-mono text-[13px] leading-[1.42] text-[#d5d9e0]',
    'bg-[hsl(var(--terminal-screen))] px-6 py-5 shadow-[inset_0_0_0_1px_rgba(255,255,255,0.04),inset_0_22px_45px_-25px_rgba(8,10,20,0.82)]',
    'outline-none focus:outline-none',
    !isFullscreen && !showTopBar && 'rounded-t-[22px]',
    !isFullscreen && !showStatusBar && 'rounded-b-[22px]',
  );
  const statusBarClasses = cn(
    'flex items-center gap-2 px-6 pb-3 text-xs text-[hsl(var(--muted-foreground))]',
    !isFullscreen && 'rounded-b-[22px]',
  );

  const containerStyle: CSSProperties & {
    '--beach-terminal-line-height': string;
    '--beach-terminal-cell-width': string;
  } = {
    fontFamily,
    fontSize,
    lineHeight: `${effectiveLineHeight}px`,
    letterSpacing: '0',
    fontKerning: 'none',
    wordSpacing: '0',
    fontVariantLigatures: 'none',
    // Prevent Chrome scroll anchoring from fighting spacer adjustments during
    // zoom/resize, which can cause off-screen rows to jump into view.
    overflowAnchor: 'none',
    '--beach-terminal-line-height': `${effectiveLineHeight}px`,
    '--beach-terminal-cell-width': `${effectiveCellWidth.toFixed(3)}px`,
  };

  const handleMatchPtyViewport = useCallback(() => {
    if (viewOnly) {
      if (IS_DEV) {
        console.warn('[beach-surfer] match host viewport blocked in view-only mode');
      }
      return;
    }
    const transport = transportRef.current;
    const targetRowsFromRef = ptyViewportRowsRef.current;
    const subscription = subscriptionRef.current;
    if (!transport || subscription === null || targetRowsFromRef == null || targetRowsFromRef <= 0) {
      return;
    }
    const clampedRows = Math.max(1, Math.min(targetRowsFromRef, MAX_VIEWPORT_ROWS));
    const snapshotNow = store.getSnapshot();
    const fallbackCols = snapshotNow.cols > 0 ? snapshotNow.cols : 80;
    const targetCols = Math.max(1, ptyColsRef.current ?? fallbackCols);
    suppressNextResizeRef.current = true;
    lastSentViewportRows.current = clampedRows;
    const label = queryLabel ?? null;
    const peerId = peerIdRef.current;
    log('match_host_viewport', {
      targetRows: clampedRows,
      targetCols,
      subscription,
      clientLabel: label ?? null,
      peerId: peerId ?? null,
      viewOnly: false,
    });
    try {
      transport.send({ type: 'resize', cols: targetCols, rows: clampedRows });
      lastSentViewportCols.current = targetCols;
      enterTailIntent('match-host-viewport');
    } catch (err) {
      if (IS_DEV) {
        console.warn('[beach-surfer] match_host_viewport send failed', err);
      }
    }
  }, [enterTailIntent, log, queryLabel, store, viewOnly]);

  const handleJumpToTail = useCallback(() => {
    const element = containerRef.current;
    if (!element) {
      enterTailIntent('jump-to-tail');
      return;
    }
    enterTailIntent('jump-to-tail');
    const target = element.scrollHeight - element.clientHeight;
    if (target < 0) {
      return;
    }
    programmaticScrollRef.current = true;
    element.scrollTop = target;
    lastScrollSnapshotRef.current = { top: element.scrollTop, height: element.clientHeight };
    if (typeof queueMicrotask === 'function') {
      queueMicrotask(() => {
        programmaticScrollRef.current = false;
      });
    } else {
      setTimeout(() => {
        programmaticScrollRef.current = false;
      }, 0);
    }
  }, [enterTailIntent]);
  const statusColor = useMemo(() => {
    switch (status) {
      case 'connected':
        return '#22c55e';
      case 'connecting':
        return '#facc15';
      case 'error':
        return '#f87171';
      case 'closed':
        return '#94a3b8';
      default:
        return '#64748b';
    }
  }, [status]);
  const hasPtyResizeTarget = ptyViewportRows != null;
  const fallbackColsForLabel = Math.max(1, ptyCols ?? (snapshot.cols > 0 ? snapshot.cols : 80));
  const matchButtonDisabled = viewOnly || !hasPtyResizeTarget || status !== 'connected';
  const matchButtonTitle = viewOnly
    ? 'Host resizing unavailable in view-only mode'
    : hasPtyResizeTarget
      ? `Match PTY size ${fallbackColsForLabel}×${ptyViewportRows}`
      : 'Host PTY size unavailable yet';
  const matchButtonAriaLabel = viewOnly
    ? 'Host resizing disabled in view-only mode'
    : hasPtyResizeTarget
      ? `Resize to host PTY size ${fallbackColsForLabel} by ${ptyViewportRows}`
      : 'Resize to host PTY size (unavailable)';
  const matchButtonClass = cn(
    'inline-flex h-3.5 w-3.5 items-center justify-center rounded-full transition focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-white/40',
    matchButtonDisabled
      ? 'cursor-not-allowed border border-[#1b2638] bg-[#101724] text-[#4c5d7f] opacity-60'
      : 'border border-[#254f8c] bg-[#2d60aa] text-[#d7e4ff] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.18)] hover:bg-[#346bc0]',
  );
  const jumpToTailAvailable =
    scrollPolicy === 'follow-tail' &&
    (!followTailDesiredState || followTailPhaseState === 'manual_scrollback');

  return (
    <div ref={wrapperRef} className={wrapperClasses}>
      <div
        className={cn(
          'pointer-events-none absolute inset-0',
          !isFullscreen && 'overflow-hidden rounded-[22px]',
        )}
      >
        <div className="absolute inset-x-0 top-0 h-28 bg-gradient-to-b from-white/12 via-white/0 to-transparent opacity-20" aria-hidden />
        <div
          className={cn('absolute inset-0 ring-1 ring-[#1f2736]/60', !isFullscreen && 'rounded-[22px]')}
          aria-hidden
        />
      </div>
      {showJoinOverlay ? (
        <JoinStatusOverlay state={joinState} message={joinMessage} isFullscreen={isFullscreen} />
      ) : null}
      {showTopBar ? (
        <header
          ref={headerRef}
          className={headerClasses}
        >
          <div className="flex items-center gap-3">
            <button
              type="button"
              onClick={() => onToggleFullscreen?.(!isFullscreen)}
              className={cn(
                'inline-flex h-3.5 w-3.5 items-center justify-center rounded-full transition focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-white/40',
                isFullscreen
                  ? 'border border-[#111827] bg-[#212838] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.06)] hover:bg-[#1d2432] text-[#a5b4d6]'
                  : 'border border-[#1a8a39] bg-[#26c547] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.2)] hover:bg-[#2cd653] text-[#0f3d1d]'
              )}
              aria-label={isFullscreen ? 'Exit full screen' : 'Enter full screen'}
              aria-pressed={isFullscreen}
            >
              <svg viewBox="0 0 12 12" className="h-2.5 w-2.5" fill="none" aria-hidden>
                {isFullscreen ? (
                  <>
                    <rect x="2.3" y="2.3" width="7.4" height="7.4" rx="1.7" stroke="currentColor" strokeWidth="1" />
                    <path d="M4.5 7.6h3" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
                  </>
                ) : (
                  <>
                    <rect x="2.3" y="2.3" width="7.4" height="7.4" rx="1.9" fill="rgba(15,61,29,0.35)" stroke="#0f3d1d" strokeWidth="1" />
                    <g transform="rotate(45 6 6)" fill="#0f3d1d">
                      <rect x="3.2" y="5.45" width="5.6" height="1.1" rx="0.4" />
                      <polygon points="3.2,6 2.2,6.55 2.2,5.45" />
                      <polygon points="8.8,6 9.8,6.55 9.8,5.45" />
                    </g>
                  </>
                )}
              </svg>
            </button>
            <button
              type="button"
              onClick={handleMatchPtyViewport}
              className={matchButtonClass}
              aria-label={matchButtonAriaLabel}
              title={matchButtonTitle}
              disabled={matchButtonDisabled}
            >
              <svg viewBox="0 0 12 12" className="h-2.5 w-2.5" fill="none" aria-hidden>
                <rect x="2.3" y="2.3" width="7.4" height="7.4" rx="1.6" stroke="currentColor" strokeWidth="0.9" />
                <path d="M3.8 6h4.4" stroke="currentColor" strokeWidth="0.9" strokeLinecap="round" />
                <path d="M6 3.6v4.8" stroke="currentColor" strokeWidth="0.9" strokeLinecap="round" />
                <path d="M6 3.6L7.1 4.7" stroke="currentColor" strokeWidth="0.85" strokeLinecap="round" strokeLinejoin="round" />
                <path d="M6 3.6L4.9 4.7" stroke="currentColor" strokeWidth="0.85" strokeLinecap="round" strokeLinejoin="round" />
                <path d="M6 8.4L7.1 7.3" stroke="currentColor" strokeWidth="0.85" strokeLinecap="round" strokeLinejoin="round" />
                <path d="M6 8.4L4.9 7.3" stroke="currentColor" strokeWidth="0.85" strokeLinecap="round" strokeLinejoin="round" />
              </svg>
            </button>
            <span className="text-[10px] font-semibold uppercase tracking-[0.5em] text-[#c0cada]">{sessionTitle}</span>
          </div>
          <div className="flex items-center gap-2 text-[10px]">
            {renderSecureBadge()}
            <span className="inline-flex items-center gap-2 rounded-full border border-white/10 px-3 py-1 text-[10px] font-semibold uppercase tracking-[0.32em] text-[#c9d2e5]">
              <span className="size-1.5 rounded-full" style={{ backgroundColor: statusColor }} aria-hidden />
              {renderStatus()}
            </span>
          </div>
        </header>
      ) : null}
      <div
        ref={containerRef}
        className={containerClasses}
        tabIndex={0}
        onKeyDown={handleKeyDown}
        onPaste={handlePaste}
        onScroll={handleScroll}
        style={containerStyle}
      >
        {showIdlePlaceholder ? (
          <IdlePlaceholder
            onConnectNotice={() => setShowIdlePlaceholder(false)}
            status={status}
          />
        ) : null}
        <div style={{ height: topPadding }} aria-hidden="true" />
        {lines.map((line) => (
          <LineRow key={line.absolute} line={line} styles={snapshot.styles} overlay={effectiveOverlay} />
        ))}
        <div style={{ height: bottomPadding }} aria-hidden="true" />
      </div>
      {showStatusBar ? (
        <footer className={statusBarClasses}>
          <span>{renderStatus()}</span>
          {jumpToTailAvailable ? (
            <button
              type="button"
              onClick={handleJumpToTail}
              className="ml-auto inline-flex items-center gap-1 rounded-full border border-[#1f2937] bg-[#19202c] px-3 py-1 text-[10px] font-semibold uppercase tracking-[0.32em] text-[#d3dbef] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.12)] transition hover:bg-[#1f2736] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-white/40"
            >
              Jump to tail
            </button>
          ) : null}
        </footer>
      ) : null}
    </div>
  );

  function handleScroll(event: UIEvent<HTMLDivElement>): void {
    const element = event.currentTarget;
    const pixelsPerRow = Math.max(1, effectiveLineHeight);
    const previousSnapshot = lastScrollSnapshotRef.current;
    const previousScrollTop = previousSnapshot.top;
    const previousHeight = previousSnapshot.height;
    const scrollEpsilon = Math.max(1, pixelsPerRow * 0.25);
    const heightChanged = Math.abs(previousHeight - element.clientHeight) > scrollEpsilon;
    const deltaPixels = element.scrollTop - previousScrollTop;
    const userScrolledUp = deltaPixels < -scrollEpsilon && !heightChanged;
    lastScrollSnapshotRef.current = { top: element.scrollTop, height: element.clientHeight };
    const approxRow = Math.max(
      snapshot.baseRow,
      snapshot.baseRow + Math.floor(element.scrollTop / pixelsPerRow),
    );
    const measuredRows = Math.max(1, Math.floor(element.clientHeight / pixelsPerRow));
    const viewportRows = Math.max(1, Math.min(measuredRows, MAX_VIEWPORT_ROWS));
    const maxTop = Math.max(snapshot.baseRow, snapshot.baseRow + totalRows - viewportRows);
    const clampedTop = Math.min(approxRow, maxTop);

    store.setViewport(clampedTop, viewportRows);
    log('scroll', {
      scrollTop: element.scrollTop,
      clientHeight: element.clientHeight,
      scrollHeight: element.scrollHeight,
      measuredRows,
      viewportRows,
      baseRow: snapshot.baseRow,
      totalRows,
      clampedTop,
    });

    const remainingPixels = Math.max(0, element.scrollHeight - (element.scrollTop + element.clientHeight));
    const hasTailPadding = tailPaddingRowsRef.current > 0;
    const atTail = hasTailPadding
      ? followTailDesiredRef.current
      : shouldReenableFollowTail(remainingPixels, pixelsPerRow);
    updateTailMetrics(remainingPixels, atTail, 'scroll');
    const nearBottom = remainingPixels <= pixelsPerRow * 2;
    const previousFollowTail = snapshot.followTail;
    const programmatic = programmaticScrollRef.current;
    if (programmatic) {
      programmaticScrollRef.current = false;
    }
    const inferredProgrammatic = programmatic || heightChanged;
    if (!inferredProgrammatic && scrollPolicy === 'follow-tail') {
      hydratingRef.current = false;
      if (followTailDesiredRef.current && userScrolledUp) {
        enterManualScrollback('user-scroll-away');
      }
    }
    const nextSnapshot = store.getSnapshot();
    trace('scroll tail decision', {
      previousFollowTail,
      requestedFollowTail: followTailDesiredRef.current,
      appliedFollowTail: nextSnapshot.followTail,
      phase: followTailPhaseRef.current,
      nearBottom,
      remainingPixels,
      atTail,
      hasTailPadding,
      tailPaddingRows: tailPaddingRowsRef.current,
      programmaticScroll: programmatic,
      heightChanged,
      inferredProgrammatic,
      lineHeight,
      lineHeightPx: pixelsPerRow,
      viewportRows,
      measuredRows,
      approxRow,
      baseRow: snapshot.baseRow,
      viewportTop: nextSnapshot.viewportTop,
      viewportHeight: nextSnapshot.viewportHeight,
      totalRows,
      firstAbsolute,
      lastAbsolute,
      scrollPolicy,
    });
    logScrollDiagnostics(
      element,
      remainingPixels,
      viewportRows,
      atTail,
      nextSnapshot,
      lines,
      firstAbsolute,
      lastAbsolute,
    );
    backfillController.maybeRequest(nextSnapshot, {
      nearBottom,
      followTailDesired: followTailDesiredRef.current,
      phase: followTailPhaseRef.current,
      tailPaddingRows: tailPaddingRowsRef.current,
    });
    emitViewportState();
  }

  function renderStatus(): string {
    if (status === 'error' && error) {
      const message = error.message ?? '';
      if (message.includes(FALLBACK_ENTITLEMENT_SUBSTRING)) {
        return 'Fallback unavailable - visit https://beach.sh to unlock Beach Auth support.';
      }
      return `Error: ${message}`;
    }
    if (status === 'connected') {
      return 'Connected';
    }
    if (status === 'connecting') {
      return 'Connecting…';
    }
    if (status === 'closed') {
      return 'Disconnected';
    }
    return 'Idle';
  }

  function renderSecureBadge(): JSX.Element | null {
    if (!secureSummary) {
      return null;
    }
    if (secureSummary.mode === 'secure') {
      return (
        <span className="inline-flex items-center gap-2 rounded-full border border-emerald-500/30 bg-emerald-500/10 px-3 py-1 text-[10px] font-semibold uppercase tracking-[0.32em] text-emerald-200">
          Secure
          {secureSummary.verificationCode ? (
            <span className="font-mono text-[11px] tracking-[0.2em] text-emerald-100/90">
              {secureSummary.verificationCode}
            </span>
          ) : null}
        </span>
      );
    }
    return (
      <span className="inline-flex items-center gap-2 rounded-full border border-amber-500/30 bg-amber-500/10 px-3 py-1 text-[10px] font-semibold uppercase tracking-[0.32em] text-amber-200">
        Plaintext
      </span>
    );
  }

  function bindTransport(transport: TerminalTransport): void {
    subscriptionRef.current = null;
    setSubscriptionVersion((v) => v + 1);
    handshakeReadyRef.current = false;
    const frameHandler = (event: Event) => {
      const frame = (event as CustomEvent<HostFrame>).detail;
      handleHostFrame(frame);
    };
    transport.addEventListener('frame', frameHandler as EventListener);
    transport.addEventListener('status', (event) => {
      const detail = (event as CustomEvent<string>).detail;
      handleStatusSignal(detail);
    });
    transport.addEventListener('secure', (event) => {
      const detail = (event as CustomEvent<SecureTransportSummary>).detail;
      setSecureSummary(detail);
    });
    transport.addEventListener('open', () => {
      markConnectionTrace('beach_terminal:data_channel_open', {
        remotePeerId: remotePeerId ?? connectionRef.current?.remotePeerId ?? null,
      });
      if (!handshakeReadyRef.current) {
        enterWaitingState();
      }
    });
    transport.addEventListener(
      'close',
      () => {
        const remote = remotePeerId ?? connectionRef.current?.remotePeerId ?? null;
        log('transport closed', { remotePeerId: remote });
        markConnectionTrace('beach_terminal:transport_close', { remotePeerId: remote });
        finishConnectionTrace('error', { stage: 'transport', reason: 'closed' });
        handshakeReadyRef.current = false;
        if (subscriptionRef.current === null && joinStateRef.current !== 'denied') {
          enterDisconnectedState();
        }
        subscriptionRef.current = null;
        setSubscriptionVersion((v) => v + 1);
        setSecureSummary(null);
        setStatus('closed');
      },
      { once: true },
    );
    transport.addEventListener('error', (event) => {
      const err = (event as any).error ?? new Error('transport error');
      const remote = remotePeerId ?? connectionRef.current?.remotePeerId ?? null;
      log('transport error', { message: err.message, remotePeerId: remote });
       markConnectionTrace('beach_terminal:transport_error', {
        message: err instanceof Error ? err.message : String(err),
        remotePeerId: remote,
      });
      finishConnectionTrace('error', { stage: 'transport', message: err.message });
      setError(err);
      setStatus('error');
    });

    const replayStatus =
      (transport as unknown as { getLastStatus?: () => string | null }).getLastStatus?.() ?? null;
    if (replayStatus) {
      handleStatusSignal(replayStatus);
    }
    if (!handshakeReadyRef.current) {
      enterWaitingState();
    }
  }

  function handleHostFrame(frame: HostFrame): void {
    captureTrace('host-frame', serializeHostFrame(frame));
    backfillController.handleFrame(frame);
    switch (frame.type) {
      case 'hello':
        trace('frame hello', frame);
        if (ptyViewportRowsRef.current !== null) {
          trace(
            'pty viewport reset on hello',
            JSON.stringify({
              previousRows: ptyViewportRowsRef.current,
              previousCols: ptyColsRef.current,
            }),
          );
          ptyViewportRowsRef.current = null;
          setPtyViewportRows((prev) => (prev === null ? prev : null));
        }
        if (ptyColsRef.current !== null) {
          trace(
            'pty columns reset on hello',
            JSON.stringify({
              previousCols: ptyColsRef.current,
            }),
          );
          ptyColsRef.current = null;
          setPtyCols((prev) => (prev === null ? prev : null));
        }
        store.reset();
        subscriptionRef.current = frame.subscription;
        setSubscriptionVersion((v) => v + 1);
        inputSeqRef.current = 0;
        store.setCursorSupport(Boolean(frame.features & FEATURE_CURSOR_SYNC));
        resetPrediction(now());
        summarizeSnapshot(store);
        hydratingRef.current = true;
        setFollowTailPhase('hydrating', 'frame-hello');
        applyFollowTailIntent('frame-hello');
        updateTailMetrics(Number.POSITIVE_INFINITY, false, 'frame-hello');
        handshakeReadyRef.current = true;
        enterApprovedState(joinStateRef.current === 'approved' ? joinMessage ?? undefined : undefined);
        markConnectionTrace('beach_terminal:hello_received', {
          subscription: frame.subscription,
        });
        finishConnectionTrace('success', {
          remotePeerId: remotePeerId ?? connectionRef.current?.remotePeerId ?? null,
          subscription: frame.subscription,
        });
        emitViewportState();
        break;
      case 'grid':
        trace('frame grid', frame);
        const previousViewportRows = ptyViewportRowsRef.current;
        {
          if (typeof window !== 'undefined') {
            try {
              console.info('[rewrite-debug][grid]', {
                sessionId,
                frameViewportRows: frame.viewportRows ?? null,
                frameHistoryRows: frame.historyRows,
                frameCols: frame.cols,
                previousPtyRows: previousViewportRows,
                previousPtyCols: ptyColsRef.current,
                subscription: subscriptionRef.current,
              });
            } catch {
              // ignore logging errors
            }
          }
          trace(
            'grid host metadata',
            JSON.stringify({
              frameViewportRows: frame.viewportRows ?? null,
              frameHistoryRows: frame.historyRows,
              frameCols: frame.cols,
              previousPtyRows: previousViewportRows,
              previousPtyCols: ptyColsRef.current,
            }),
          );
          if (
            typeof frame.viewportRows === 'number' &&
            frame.viewportRows > 0 &&
            frame.cols > 0 &&
            frame.viewportRows >= 80 &&
            frame.cols <= 80 &&
            frame.viewportRows >= frame.cols * 1.2
          ) {
            logSizing('grid host dimension anomaly', {
              frameViewportRows: frame.viewportRows,
              frameCols: frame.cols,
              historyRows: frame.historyRows,
              previousViewportRows,
              previousCols: ptyColsRef.current,
              subscription: subscriptionRef.current,
            });
          }
          const nextCols = Math.max(1, frame.cols);
          ptyColsRef.current = nextCols;
          setPtyCols((prev) => (prev === nextCols ? prev : nextCols));
        }
        const rawViewportRows =
          typeof frame.viewportRows === 'number' && frame.viewportRows > 0
            ? frame.viewportRows
            : previousViewportRows && previousViewportRows > 0
              ? previousViewportRows
              : null;
        if (rawViewportRows > 0) {
          const clampedRows = Math.max(1, Math.min(rawViewportRows, MAX_VIEWPORT_ROWS));
          trace(
            'grid host viewport applied',
            JSON.stringify({
              rawViewportRows,
              clampedRows,
              previousPtyRows: ptyViewportRowsRef.current,
            }),
          );
          ptyViewportRowsRef.current = clampedRows;
          setPtyViewportRows((prev) => (prev === clampedRows ? prev : clampedRows));
        } else {
          trace(
            'grid host viewport missing',
            JSON.stringify({
              rawViewportRows,
              frameViewportRows: frame.viewportRows ?? null,
              frameHistoryRows: frame.historyRows,
            }),
          );
        }
        if (
          typeof window !== 'undefined' &&
          frame.historyRows > 0 &&
          typeof frame.viewportRows !== 'number'
        ) {
          try {
            console.info('[terminal][diag] history-rows-mismatch', {
              sessionId,
              frameViewportRows: frame.viewportRows ?? null,
              appliedViewportRows: rawViewportRows,
              previousViewportRows,
              historyRows: frame.historyRows,
              cols: frame.cols,
            });
          } catch {
            // ignore logging issues
          }
        }
        const preGridSnapshot = store.getSnapshot();
        const hydratedRows = preGridSnapshot.rows.length;
        const hydratedBaseRow = preGridSnapshot.baseRow;
        const hydratedCols = preGridSnapshot.cols;
        const hydratedViewportTop = preGridSnapshot.viewportTop;
        const hasHydratedHistory = hydratedRows > 0;
        const handshakeHasHistory = frame.historyRows > 0;
        const nextBaseRow = (() => {
          if (hasHydratedHistory) {
            if (!handshakeHasHistory) {
              return hydratedBaseRow;
            }
            return Math.min(hydratedBaseRow, frame.baseRow);
          }
          if (handshakeHasHistory) {
            return frame.baseRow;
          }
          return preGridSnapshot.baseRow;
        })();
        if (preGridSnapshot.baseRow !== nextBaseRow) {
          store.setBaseRow(nextBaseRow);
        }
        const hydratedEnd = hydratedBaseRow + hydratedRows;
        const handshakeEnd = handshakeHasHistory ? frame.baseRow + frame.historyRows : hydratedEnd;
        const unionEnd = Math.max(hydratedEnd, handshakeEnd);
        const desiredTotalRows = Math.max(hydratedRows, unionEnd - nextBaseRow);
        const desiredCols = Math.max(hydratedCols || 0, frame.cols);
        store.setGridSize(desiredTotalRows, desiredCols);
        store.setFollowTail(false);
        hydratingRef.current = true;
        setFollowTailPhase('hydrating', 'frame-grid');
        applyFollowTailIntent('frame-grid');
        {
          const deviceViewport = Math.max(
            1,
            Math.min(lastMeasuredViewportRows.current, MAX_VIEWPORT_ROWS),
          );
          const viewportTopCandidate = hasHydratedHistory
            ? hydratedViewportTop
            : handshakeHasHistory
              ? frame.baseRow
              : preGridSnapshot.viewportTop;
          const maxViewportTop = Math.max(nextBaseRow, unionEnd - deviceViewport);
          const clampedViewportTop = Math.min(
            Math.max(viewportTopCandidate, nextBaseRow),
            maxViewportTop,
          );
          store.setViewport(clampedViewportTop, deviceViewport);
          if (lastSentViewportRows.current === 0) {
            lastSentViewportRows.current = deviceViewport;
          }
          if (lastSentViewportCols.current == null && frame.cols > 0) {
            lastSentViewportCols.current = Math.max(1, Math.min(frame.cols, MAX_VIEWPORT_COLS));
          }
          suppressNextResizeRef.current = true;
          log('grid frame', {
            baseRow: nextBaseRow,
            historyRows: desiredTotalRows,
            handshakeBaseRow: frame.baseRow,
            handshakeHistoryRows: frame.historyRows,
            hydratedBaseRow,
            hydratedRows,
            cols: frame.cols,
            serverViewport: frame.viewportRows ?? null,
            deviceViewport,
            viewportTop: clampedViewportTop,
            historyEnd: unionEnd,
          });
        }
        resetPrediction(now());
        emitViewportState();
        if (subscriptionRef.current !== null) {
    triggerViewportRecalc('grid-frame', true);
        }
        break;
      case 'snapshot':
      case 'delta':
      case 'history_backfill': {
        const authoritative = frame.type === 'snapshot' || frame.type === 'history_backfill';
        log(`frame ${frame.type}`, { updates: frame.updates.length, authoritative });
        trace('frame updates', {
          type: frame.type,
          updates: frame.updates.map((update) => update.type),
          cursor: frame.cursor ?? null,
        });
        predictiveLog('server_frame', { frame: frame.type, updates: frame.updates.length, authoritative });
        store.applyUpdates(frame.updates, {
          authoritative,
          origin: frame.type,
          cursor: frame.cursor ?? null,
        });
        if (frame.type === 'history_backfill') {
          backfillController.finalizeHistoryBackfill(frame);
        }
        summarizeSnapshot(store);
        exitHydration(`frame-${frame.type}`);
        const current = store.getSnapshot();
        backfillController.maybeRequest(current, {
          nearBottom: current.followTail,
          followTailDesired: followTailDesiredRef.current,
          phase: followTailPhaseRef.current,
          tailPaddingRows: tailPaddingRowsRef.current,
        });
        break;
      }
      case 'snapshot_complete':
        exitHydration('frame-snapshot-complete');
        break;
      case 'input_ack': {
        const timestamp = now();
        predictiveLog('server_frame', { frame: 'input_ack', seq: frame.seq });
        store.ackPrediction(frame.seq, timestamp);
        recordPredictionAck(frame.seq, timestamp);
        break;
      }
      case 'cursor':
        trace('frame cursor', frame.cursor);
        store.applyCursorFrame(frame.cursor);
        summarizeSnapshot(store);
        break;
      case 'heartbeat':
        trace('frame heartbeat', frame.seq);
        break;
      case 'shutdown':
        trace('frame shutdown');
        if (ptyViewportRowsRef.current !== null) {
          ptyViewportRowsRef.current = null;
          setPtyViewportRows((prev) => (prev === null ? prev : null));
        }
        if (ptyColsRef.current !== null) {
          ptyColsRef.current = null;
          setPtyCols((prev) => (prev === null ? prev : null));
        }
        setStatus('closed');
        emitViewportState();
        break;
      default:
        break;
    }
  }

  function logScrollDiagnostics(
    element: HTMLDivElement,
    remainingPixels: number,
    viewportRows: number,
    requestedFollowTail: boolean,
    snapshot: TerminalGridSnapshot,
    renderLines: RenderLine[],
    firstAbsolute: number,
    lastAbsolute: number,
  ): void {
    if (!(typeof window !== 'undefined' && window.__BEACH_TRACE)) {
      return;
    }
    const appliedFollowTail = snapshot.followTail;
    const rowHeight = Math.max(1, effectiveLineHeight);
    const rowElements = element.querySelectorAll<HTMLDivElement>('.xterm-row');
    if (rowElements.length === 0) {
      trace('scroll diagnostics', {
        renderedRows: 0,
        requestedFollowTail,
        appliedFollowTail,
        remainingPixels,
        viewportRows,
        scrollHeight: element.scrollHeight,
        clientHeight: element.clientHeight,
        scrollTop: element.scrollTop,
        lineHeight: rowHeight,
      });
      return;
    }

    const firstRect = rowElements[0]!.getBoundingClientRect();
    const middleIndex = Math.min(rowElements.length - 1, Math.floor(rowElements.length / 2));
    const middleRect = rowElements[middleIndex]!.getBoundingClientRect();
    const lastRect = rowElements[rowElements.length - 1]!.getBoundingClientRect();
    const averageHeight = rowElements.length > 1
      ? (lastRect.bottom - firstRect.top) / (rowElements.length - 1)
      : firstRect.height;
    const approximateRowsFromScrollHeight = element.scrollHeight / rowHeight;
    const approximateRowsFromAverage = element.scrollHeight / averageHeight;

    const summarizeLine = (line: RenderLine | undefined) => {
      if (!line) {
        return null;
      }
      const text = line.cells?.map((cell) => (cell.char === '\u00a0' ? ' ' : cell.char)).join('').trimEnd();
      return {
        absolute: line.absolute,
        kind: line.kind,
        text,
        cursorCol: line.cursorCol ?? null,
      };
    };

    const topLine = summarizeLine(renderLines[0]);
    const middleLine = summarizeLine(renderLines[middleIndex]);
    const bottomLine = summarizeLine(renderLines[renderLines.length - 1]);

    trace('scroll diagnostics', {
      renderedRows: rowElements.length,
      requestedFollowTail,
      appliedFollowTail: snapshot.followTail,
      remainingPixels,
      viewportRows,
      scrollHeight: element.scrollHeight,
      clientHeight: element.clientHeight,
      scrollTop: element.scrollTop,
      firstRowHeight: Number(firstRect.height.toFixed(3)),
      middleRowHeight: Number(middleRect.height.toFixed(3)),
      lastRowHeight: Number(lastRect.height.toFixed(3)),
      averageRowHeight: Number(averageHeight.toFixed(3)),
      approximateRowsFromScrollHeight: Number(approximateRowsFromScrollHeight.toFixed(3)),
      approximateRowsFromAverage: Number(approximateRowsFromAverage.toFixed(3)),
      lineHeight: rowHeight,
      firstAbsolute,
      lastAbsolute,
      topLine,
      middleLine,
      bottomLine,
      viewportTop: snapshot.viewportTop,
      viewportHeight: snapshot.viewportHeight,
      baseRow: snapshot.baseRow,
    });

    const summaryParts = [
      `followTail=${snapshot.followTail}`,
      `requested=${requestedFollowTail}`,
      `remaining=${remainingPixels.toFixed(2)}`,
      `viewportTop=${snapshot.viewportTop}`,
      topLine ? `first=${topLine.absolute}:${truncate(topLine.text)}` : 'first=?',
      bottomLine ? `last=${bottomLine.absolute}:${truncate(bottomLine.text)}` : 'last=?',
    ];
    console.debug('[beach-trace][terminal] scroll summary', summaryParts.join(' | '));
  }
}

function truncate(text: string | undefined | null, max = 48): string {
  if (!text) {
    return '';
  }
  if (text.length <= max) {
    return text;
  }
  return `${text.slice(0, max)}…`;
}

function findLastContentAbsolute(snapshot: TerminalGridSnapshot): number | null {
  const rows = snapshot.rows;
  const cursorRow = snapshot.cursorRow ?? null;
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    const slot = rows[index];
    if (!slot || slot.kind !== 'loaded') {
      continue;
    }
    if (rowHasVisibleContent(slot.cells)) {
      return slot.absolute;
    }
    if (cursorRow !== null && slot.absolute === cursorRow) {
      return slot.absolute;
    }
    const predictions = snapshot.predictionsForRow(slot.absolute);
    if (predictions.length > 0) {
      return slot.absolute;
    }
  }
  if (cursorRow !== null && cursorRow >= snapshot.baseRow) {
    return cursorRow;
  }
  return null;
}

function rowHasVisibleContent(cells: CellState[]): boolean {
  for (const cell of cells) {
    if (cell.char !== ' ' || cell.styleId !== 0) {
      return true;
    }
  }
  return false;
}

interface RenderCell {
  char: string;
  styleId: number;
  predicted?: boolean;
}

function computeVisibleWidth(cells: RenderCell[]): number {
  for (let index = cells.length - 1; index >= 0; index -= 1) {
    const cell = cells[index];
    if (!cell) {
      continue;
    }
    if (cell.char !== ' ' || cell.styleId !== 0) {
      return index + 1;
    }
  }
  return 0;
}

interface RenderLine {
  absolute: number;
  kind: 'loaded' | 'pending' | 'missing';
  cells?: RenderCell[];
  cursorCol?: number | null;
  predictedCursorCol?: number | null;
}

export function buildLines(
  snapshot: TerminalGridSnapshot,
  limit = Number.POSITIVE_INFINITY,
  overlay: PredictionOverlayState = DEFAULT_PREDICTION_OVERLAY,
): RenderLine[] {
  const rows = snapshot.visibleRows(limit);
  if (rows.length === 0) {
    return [];
  }

  const placeholderWidth = Math.max(1, snapshot.cols || 80);
  const lines: RenderLine[] = [];

  for (const row of rows) {
    if (row.kind === 'loaded') {
      const cells: RenderCell[] = row.cells.map((cell) => ({
        char: cell.char ?? ' ',
        styleId: cell.styleId ?? 0,
      }));
      const visibleWidth = computeVisibleWidth(cells);
      const handshakeActive =
        overlay.visible &&
        snapshot.cursorSeq === null &&
        snapshot.cursorVisible === false &&
        snapshot.cursorRow === row.absolute;
      let handshakeProjectionCol: number | null = null;
      if (overlay.visible) {
        const predictions = snapshot.predictionsForRow(row.absolute);
        if (predictions.length > 0) {
          for (const { col, cell: prediction } of predictions) {
            let targetCol = col;
            if (handshakeActive) {
              const offsetFromContent = col - visibleWidth;
              if (visibleWidth >= 0 && offsetFromContent >= PREDICTION_HANDSHAKE_OFFSET_THRESHOLD) {
                if (handshakeProjectionCol === null) {
                  handshakeProjectionCol = Math.max(visibleWidth, 0);
                }
                targetCol = handshakeProjectionCol;
                handshakeProjectionCol += 1;
                trace('buildLines: remapped handshake prediction', {
                  row: row.absolute,
                  originalCol: col,
                  remappedCol: targetCol,
                  visibleWidth,
                });
              }
            }
            while (cells.length <= targetCol) {
              cells.push({ char: ' ', styleId: 0 });
            }
            const existing = cells[targetCol];
            const predictionChar = prediction.char ?? ' ';
            // Only overlay prediction if the cell is empty/whitespace
            // If server has sent authoritative content, don't replace it with prediction
            if (existing && existing.char !== ' ' && existing.char !== predictionChar) {
              // Server has authoritative content that differs from prediction - skip this prediction
              trace('buildLines: skipping prediction due to authoritative content', {
                row: row.absolute,
                col: targetCol,
                existing: existing.char,
                prediction: predictionChar,
              });
              continue;
            }
            cells[targetCol] = {
              char: predictionChar,
              styleId: existing?.styleId ?? 0,
              predicted: true,
            };
            trace('buildLines: applied prediction', {
              row: row.absolute,
              col: targetCol,
              char: predictionChar,
              overlayVisible: overlay.visible,
              viewportTop: snapshot.viewportTop,
              cursorRow: snapshot.cursorRow,
              cursorCol: snapshot.cursorCol,
            });
          }
        }
      }
      let cursorCol: number | null = null;
      if (snapshot.cursorVisible && snapshot.cursorRow === row.absolute && snapshot.cursorCol !== null) {
        const raw = Math.floor(Math.max(snapshot.cursorCol, 0));
        cursorCol = Number.isFinite(raw) ? raw : null;
      }
      let predictedCursorCol: number | null = null;
      if (
        overlay.visible &&
        snapshot.predictedCursor &&
        snapshot.predictedCursor.row === row.absolute &&
        Number.isFinite(snapshot.predictedCursor.col)
      ) {
        predictedCursorCol = Math.max(0, Math.floor(snapshot.predictedCursor.col));
      }
      lines.push({ absolute: row.absolute, kind: 'loaded', cells, cursorCol, predictedCursorCol });
      continue;
    }
    const fillChar = row.kind === 'pending' ? '·' : ' ';
    const width = row.kind === 'pending' ? placeholderWidth : placeholderWidth;
    const cells = Array.from({ length: width }, () => ({ char: fillChar, styleId: 0 }));
    lines.push({ absolute: row.absolute, kind: row.kind, cells });
  }

  const buildLinesPayload = {
    limit,
    followTail: snapshot.followTail,
    viewportTop: snapshot.viewportTop,
    viewportHeight: snapshot.viewportHeight,
    baseRow: snapshot.baseRow,
    rowKinds: rows.map((row) => row.kind),
    absolutes: rows.map((row) => row.absolute),
    lineKinds: lines.map((line) => line.kind),
    lineAbsolutes: lines.map((line) => line.absolute),
  };
  trace('buildLines result', buildLinesPayload);
  captureTrace('buildLines', buildLinesPayload);
  if (typeof console !== 'undefined') {
    try {
      console.info('[beach-trace][terminal][buildLines result]', JSON.stringify(buildLinesPayload));
    } catch {
      console.info('[beach-trace][terminal][buildLines result]', buildLinesPayload);
    }
  }
  if (typeof window !== 'undefined' && Array.isArray((window as typeof window & { __BEACH_TRACE_HISTORY?: unknown[] }).__BEACH_TRACE_HISTORY)) {
    (window as typeof window & { __BEACH_TRACE_HISTORY: unknown[] }).__BEACH_TRACE_HISTORY.push({
      scope: 'terminal',
      event: 'buildLines result',
      payload: {
        limit,
        followTail: snapshot.followTail,
        viewportTop: snapshot.viewportTop,
        viewportHeight: snapshot.viewportHeight,
        baseRow: snapshot.baseRow,
        rowKinds: rows.map((row) => row.kind),
        absolutes: rows.map((row) => row.absolute),
        lineKinds: lines.map((line) => line.kind),
        lineAbsolutes: lines.map((line) => line.absolute),
      },
    });
  }

  return lines;
}

function JoinStatusOverlay({
  state,
  message,
  isFullscreen,
}: {
  state: JoinOverlayState;
  message: string | null;
  isFullscreen: boolean;
}): JSX.Element | null {
  if (state === 'idle') {
    return null;
  }
  const text = message ?? JOIN_WAIT_DEFAULT;
  const showSpinner = state === 'connecting' || state === 'waiting';
  const badgeText = state === 'approved' ? 'OK' : state === 'denied' ? 'NO' : 'OFF';

  return (
    <div
      className={cn(
        'pointer-events-none absolute inset-0 z-20 flex items-center justify-center bg-[#05070b]/80 backdrop-blur-sm',
        !isFullscreen && 'rounded-[22px]',
      )}
    >
      <div className="pointer-events-auto flex w-[min(420px,90%)] flex-col items-center gap-3 rounded-lg border border-white/10 bg-[#111827]/95 px-6 py-5 text-center text-sm text-slate-200 shadow-2xl">
        {showSpinner ? (
          <div className="h-8 w-8 animate-spin rounded-full border-2 border-white/40 border-t-transparent" />
        ) : (
          <div className="flex h-8 w-8 items-center justify-center rounded-full border border-white/30 text-xs font-semibold uppercase tracking-wide text-white/80">
            {badgeText}
          </div>
        )}
        <p className="font-medium tracking-wide text-white/90">{text}</p>
      </div>
    </div>
  );
}

function LineRow({
  line,
  styles,
  overlay,
}: {
  line: RenderLine;
  styles: Map<number, StyleDefinition>;
  overlay: PredictionOverlayState;
}): JSX.Element {
  if (!line.cells || line.kind !== 'loaded') {
    const text = line.cells?.map((cell) => cell.char).join('') ?? '';
    const className = cn('xterm-row', line.kind === 'pending' ? 'opacity-60' : undefined);
    return <div className={className}>{text}</div>;
  }

  const cursorCol = line.cursorCol ?? null;
  const overlayVisible = overlay.visible;
  const overlayUnderline = overlay.underline;
  const predictedCursorCol = overlayVisible ? line.predictedCursorCol ?? null : null;
  const baseStyleDef = styles.get(0) ?? { id: 0, fg: 0, bg: 0, attrs: 0 };

  return (
    <div className="xterm-row">
      {line.cells.map((cell, index) => {
        const styleDef = styles.get(cell.styleId);
        const isCursor = cursorCol !== null && cursorCol === index;
        const style = styleDef ? styleFromDefinition(styleDef, isCursor) : undefined;
        const predictedCell = overlayVisible && cell.predicted === true;
        const isPredictedCursor =
          overlayVisible && predictedCursorCol !== null && predictedCursorCol === index && !isCursor;
        const predicted = predictedCell || isPredictedCursor;
        const underline = predicted && overlayUnderline;
        const char = cell.char === ' ' ? NBSP : cell.char;
        return (
          <span
            key={index}
            style={style}
            data-predicted={predicted || undefined}
            data-predicted-underline={underline || undefined}
            data-predicted-cursor={isPredictedCursor || undefined}
          >
            {char}
          </span>
        );
      })}
      {cursorCol !== null && cursorCol >= line.cells.length ? (
        <span key="cursor" style={styleFromDefinition(baseStyleDef, true)}>{NBSP}</span>
      ) : null}
      {overlayVisible && predictedCursorCol !== null && predictedCursorCol >= line.cells.length ? (
        <span
          key="predicted-cursor"
          style={styleFromDefinition(baseStyleDef, false)}
          data-predicted
          data-predicted-underline={overlayUnderline || undefined}
          data-predicted-cursor
        >
          {NBSP}
        </span>
      ) : null}
    </div>
  );
}

function computeLineHeight(fontSize: number): number {
  return Math.round(fontSize * 1.4);
}

function measureFontGlyphMetrics(
  fontFamily: string,
  fontSize: number,
): { cellWidth: number; lineHeight: number } | null {
  if (typeof document === 'undefined') {
    return null;
  }
  const sampleText = 'MMMMMMMMMM';
  const measure = document.createElement('span');
  measure.textContent = sampleText;
  measure.style.position = 'absolute';
  measure.style.visibility = 'hidden';
  measure.style.whiteSpace = 'pre';
  measure.style.fontFamily = fontFamily;
  measure.style.fontSize = `${fontSize}px`;
  measure.style.lineHeight = 'normal';
  measure.style.fontKerning = 'none';
  measure.style.margin = '0';
  measure.style.padding = '0';
  measure.style.letterSpacing = '0';
  document.body.appendChild(measure);
  const rect = measure.getBoundingClientRect();
  measure.remove();
  if (!Number.isFinite(rect.width) || rect.width <= 0 || !Number.isFinite(rect.height) || rect.height <= 0) {
    logCellMetric('font-metrics-miss', { fontFamily, fontSize, width: rect.width, height: rect.height });
    return null;
  }
  const widthPerChar = rect.width / sampleText.length;
  logCellMetric('font-metrics', {
    fontFamily,
    fontSize,
    sampleWidth: rect.width,
    sampleHeight: rect.height,
    glyphWidth: widthPerChar,
  });
  return { cellWidth: widthPerChar, lineHeight: rect.height };
}

export function shouldReenableFollowTail(remainingPixels: number, lineHeightPx: number): boolean {
  const tolerance = Math.max(1, Math.ceil(lineHeightPx * 2));
  return remainingPixels <= tolerance;
}

const DEFAULT_FOREGROUND = '#e2e8f0';
const DEFAULT_BACKGROUND = 'hsl(var(--terminal-screen))';
const NBSP = '\u00A0';
const DEFAULT_TERMINAL_COLS = 80;
const BASE_TERMINAL_FONT_SIZE = 14;
const BASE_TERMINAL_CELL_WIDTH = 8;

function styleFromDefinition(def: StyleDefinition, highlightCursor = false): CSSProperties {
  const style: CSSProperties = {};
  const attrs = (def.attrs ?? 0) | (highlightCursor ? 1 << 4 : 0);

  let fg = decodeColor(def.fg);
  let bg = decodeColor(def.bg);

  if (attrs & (1 << 4)) {
    const fallbackFg = fg ?? DEFAULT_FOREGROUND;
    const fallbackBg = bg ?? DEFAULT_BACKGROUND;
    fg = fallbackBg;
    bg = fallbackFg;
  }

  if (fg) {
    style.color = fg;
  }
  if (bg) {
    style.backgroundColor = bg;
  }

  if (attrs & (1 << 0)) {
    style.fontWeight = 'bold';
  }
  if (attrs & (1 << 1)) {
    style.fontStyle = 'italic';
  }
  if (attrs & (1 << 2)) {
    style.textDecoration = appendTextDecoration(style.textDecoration, 'underline');
  }
  if (attrs & (1 << 3)) {
    style.textDecoration = appendTextDecoration(style.textDecoration, 'line-through');
  }
  if (attrs & (1 << 5)) {
    style.animation = 'beach-terminal-blink 1s steps(1, start) infinite';
  }
  if (attrs & (1 << 6)) {
    style.opacity = style.opacity ? Number(style.opacity) * 0.6 : 0.6;
  }
  if (attrs & (1 << 7)) {
    style.visibility = 'hidden';
  }

  return style;
}

function appendTextDecoration(existing: string | number | undefined, value: string): string {
  if (existing === undefined) {
    return value;
  }
  const current = String(existing);
  if (current.includes(value)) {
    return current;
  }
  return `${current} ${value}`.trim();
}

function decodeColor(packed: number): string | undefined {
  const mode = (packed >>> 24) & 0xff;
  if (mode === 0) {
    return undefined;
  }
  if (mode === 1) {
    return colorFromIndexed(packed & 0xff);
  }
  if (mode === 2) {
    const r = (packed >>> 16) & 0xff;
    const g = (packed >>> 8) & 0xff;
    const b = packed & 0xff;
    return `rgb(${r}, ${g}, ${b})`;
  }
  return undefined;
}

function colorFromIndexed(index: number): string {
  const ansi16 = [
    '#000000', '#800000', '#008000', '#808000', '#000080', '#800080', '#008080', '#c0c0c0',
    '#808080', '#ff0000', '#00ff00', '#ffff00', '#0000ff', '#ff00ff', '#00ffff', '#ffffff',
  ];
  if (index < ansi16.length) {
    return ansi16[index]!;
  }
  if (index >= 16 && index <= 231) {
    const value = index - 16;
    const r = Math.floor(value / 36);
    const g = Math.floor((value % 36) / 6);
    const b = value % 6;
    const component = (n: number) => (n === 0 ? 0 : 55 + n * 40);
    return `rgb(${component(r)}, ${component(g)}, ${component(b)})`;
  }
  if (index >= 232 && index <= 255) {
    const level = 8 + (index - 232) * 10;
    return `rgb(${level}, ${level}, ${level})`;
  }
  return DEFAULT_FOREGROUND;
}

function IdlePlaceholder({
  onConnectNotice,
  status,
}: {
  onConnectNotice: () => void;
  status: TerminalStatus;
}): JSX.Element {
  useEffect(() => {
    if (status === 'connected') {
      onConnectNotice();
    }
  }, [status, onConnectNotice]);
  return (
    <div className="pointer-events-none flex min-h-[300px] flex-col items-center justify-center gap-2 bg-gradient-to-br from-[#0a101b]/92 via-[#0d1421]/92 to-[#05070b]/94 px-6 py-10 text-center text-[13px] font-mono text-[#8f9ab5]">
      <div className="rounded-full border border-white/10 bg-[#141a28]/90 px-3 py-1 text-[11px] uppercase tracking-[0.4em] text-white/70">Terminal idle</div>
      <p className="text-xs text-white/40">Awaiting connection</p>
    </div>
  );
}
ensureTraceCaptureHelpers();
