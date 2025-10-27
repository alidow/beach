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
import type { ServerMessage } from '../transport/signaling';
import type { SecureTransportSummary } from '../transport/webrtc';

export type TerminalStatus = 'idle' | 'connecting' | 'connected' | 'error' | 'closed';

type JoinOverlayState =
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
  className?: string;
  fontFamily?: string;
  fontSize?: number;
  showStatusBar?: boolean;
  isFullscreen?: boolean;
  onToggleFullscreen?: (next: boolean) => void;
  showTopBar?: boolean;
  fallbackOverrides?: FallbackOverrides;
}

export function BeachTerminal(props: BeachTerminalProps): JSX.Element {
  const MAX_VIEWPORT_ROWS = 512;
  const {
    sessionId,
    baseUrl,
    passcode,
    autoConnect = false,
    onStatusChange,
    transport: providedTransport,
    store: providedStore,
    fallbackOverrides,
    className,
    fontFamily = "'SFMono-Regular', 'Menlo', 'Consolas', monospace",
    fontSize = 14,
    showStatusBar = true,
    isFullscreen = false,
    onToggleFullscreen,
    showTopBar = true,
  } = props;

  const store = useMemo(() => providedStore ?? createTerminalStore(), [providedStore]);
  if (IS_DEV && typeof window !== 'undefined') {
    (window as any).beachStore = store;
  }
  const snapshot = useTerminalSnapshot(store);
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const headerRef = useRef<HTMLDivElement | null>(null);
  const transportRef = useRef<TerminalTransport | null>(providedTransport ?? null);
  const predictionUxRef = useRef<PredictionUx>(new PredictionUx());
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
  const [status, setStatus] = useState<TerminalStatus>(
    providedTransport ? 'connected' : 'idle',
  );
  const [error, setError] = useState<Error | null>(null);
  const [secureSummary, setSecureSummary] = useState<SecureTransportSummary | null>(null);
  const [showIdlePlaceholder, setShowIdlePlaceholder] = useState(true);
  const [, setHeaderHeight] = useState<number>(0);
  const [activeConnection, setActiveConnection] = useState<BrowserTransportConnection | null>(null);
  const [peerId, setPeerId] = useState<string | null>(null);
  const [remotePeerId, setRemotePeerId] = useState<string | null>(null);
  const [joinState, setJoinState] = useState<JoinOverlayState>('idle');
  const joinStateRef = useRef<JoinOverlayState>('idle');
  const [joinMessage, setJoinMessage] = useState<string | null>(null);
  const [predictionOverlay, setPredictionOverlay] = useState<PredictionOverlayState>({
    visible: false,
    underline: false,
  });
  const [ptyViewportRows, setPtyViewportRows] = useState<number | null>(null);
  const ptyViewportRowsRef = useRef<number | null>(null);
  const [ptyCols, setPtyCols] = useState<number | null>(null);
  const ptyColsRef = useRef<number | null>(null);
  const effectiveOverlay = useMemo(() => {
    if (predictionOverlay.visible || !snapshot.hasPredictions) {
      return predictionOverlay;
    }
    return { ...predictionOverlay, visible: true };
  }, [predictionOverlay, snapshot.hasPredictions]);
  const joinTimersRef = useRef<{ short?: number; long?: number; hide?: number }>({});
  const peerIdRef = useRef<string | null>(null);
  const handshakeReadyRef = useRef(false);
  const markConnectionTrace = useCallback(
    (name: string, extra: Record<string, unknown> = {}) => {
      connectionTraceRef.current?.mark(name, extra);
    },
    [],
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
    return () => {
      clearJoinTimers();
    };
  }, [clearJoinTimers]);
  useEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }
    let raf = 0;
    const step = () => {
      const timestamp = now();
      store.pruneAckedPredictions(timestamp, PREDICTION_ACK_GRACE_MS);
      const update = predictionUxRef.current.tick(timestamp);
      if (update) {
        setPredictionOverlay(update);
      }
      raf = window.requestAnimationFrame(step);
    };
    raf = window.requestAnimationFrame(step);
    return () => {
      if (raf) {
        window.cancelAnimationFrame(raf);
      }
    };
  }, [store]);
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

  const enterWaitingState = useCallback(
    (message?: string) => {
      handshakeReadyRef.current = false;
      const trimmed = message?.trim();
      const effective = trimmed && trimmed.length > 0 ? trimmed : JOIN_WAIT_INITIAL;
      setJoinState('waiting');
      setJoinMessage(effective);
      clearJoinTimers();
      if (!trimmed || trimmed.length === 0) {
        joinTimersRef.current.short = window.setTimeout(() => {
          setJoinMessage((current) => {
            if (!current) {
              return current;
            }
            if (current === JOIN_WAIT_INITIAL || current === JOIN_WAIT_DEFAULT) {
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
              return JOIN_WAIT_HINT_TWO;
            }
            return current;
          });
        }, 30_000);
      }
    },
    [clearJoinTimers],
  );

  const enterApprovedState = useCallback(
    (message?: string) => {
      handshakeReadyRef.current = true;
      const trimmed = message?.trim();
      const effective = trimmed && trimmed.length > 0 ? trimmed : JOIN_APPROVED_MESSAGE;
      setJoinState('approved');
      setJoinMessage(effective);
      clearJoinTimers();
      joinTimersRef.current.hide = window.setTimeout(() => {
        setJoinState('idle');
        setJoinMessage(null);
        joinTimersRef.current.hide = undefined;
      }, JOIN_OVERLAY_HIDE_DELAY_MS);
    },
    [clearJoinTimers],
  );

  const enterDeniedState = useCallback(
    (message?: string) => {
      handshakeReadyRef.current = false;
      const trimmed = message?.trim();
      const effective = trimmed && trimmed.length > 0 ? trimmed : JOIN_DENIED_MESSAGE;
      setJoinState('denied');
      setJoinMessage(effective);
      clearJoinTimers();
      joinTimersRef.current.hide = window.setTimeout(() => {
        setJoinState('idle');
        setJoinMessage(null);
        joinTimersRef.current.hide = undefined;
      }, JOIN_OVERLAY_HIDE_DELAY_MS);
    },
    [clearJoinTimers],
  );

  const enterDisconnectedState = useCallback(() => {
    handshakeReadyRef.current = false;
    setJoinState('disconnected');
    setJoinMessage(JOIN_DISCONNECTED_MESSAGE);
    clearJoinTimers();
    joinTimersRef.current.hide = window.setTimeout(() => {
      setJoinState('idle');
      setJoinMessage(null);
      joinTimersRef.current.hide = undefined;
    }, JOIN_OVERLAY_HIDE_DELAY_MS);
  }, [clearJoinTimers]);

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
  }, [status, onStatusChange]);
  const lines = useMemo(() => buildLines(snapshot, 600, effectiveOverlay), [snapshot, effectiveOverlay]);
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
  const effectiveLineHeight = measuredLineHeight > 0 ? measuredLineHeight : lineHeight;
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
    const container = containerRef.current;
    if (!container) {
      return;
    }
    let raf = -1;
    const measure = () => {
      const row = container.querySelector<HTMLDivElement>('.xterm-row');
      if (!row) {
        return;
      }
      const rect = row.getBoundingClientRect();
      const next = rect.height;
      if (!Number.isFinite(next) || next <= 0) {
        return;
      }
      setMeasuredLineHeight((prev) => (Math.abs(prev - next) > 0.1 ? next : prev));
    };
    raf = window.requestAnimationFrame(measure);
    return () => {
      if (raf !== -1) {
        window.cancelAnimationFrame(raf);
      }
    };
  }, [lines.length, fontFamily, fontSize]);

  useEffect(() => {
    transportRef.current = providedTransport ?? null;
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
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [providedTransport]);

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
      setActiveConnection(null);
      setSecureSummary(null);
      setPeerId(null);
      setRemotePeerId(null);
      handshakeReadyRef.current = false;
      clearJoinTimers();
      setJoinState('idle');
      setJoinMessage(null);
      finishConnectionTrace('cancelled', { reason: 'cleanup' });
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoConnect, sessionId, baseUrl, passcode, queryLabel]);

  useEffect(() => {
    if (!wrapperRef.current || !containerRef.current) {
      return;
    }
    const wrapper = wrapperRef.current;
    const container = containerRef.current;
    const observer = new ResizeObserver(() => {
      // Measure the container's actual viewport size
      // This is the fixed available space, not affected by internal padding
      const containerRect = container.getBoundingClientRect();
      const viewportHeight = containerRect.height;
      const rowHeight = Math.max(1, effectiveLineHeight);
      const measured = Math.max(1, Math.floor(viewportHeight / rowHeight));
      // Clamp to the physical window height so we never report more rows than
      // the screen can actually display. Using innerHeight keeps the loop from
      // chasing scrollHeight growth when content expands.
      const windowRows = typeof window !== 'undefined'
        ? Math.max(1, Math.floor(window.innerHeight / rowHeight))
        : MAX_VIEWPORT_ROWS;
      const fallbackRows = Math.max(1, Math.min(windowRows, MAX_VIEWPORT_ROWS));
      const viewportRows = Math.max(1, Math.min(measured, fallbackRows));
      lastMeasuredViewportRows.current = viewportRows;
      const maxHeightPx = fallbackRows * rowHeight;
      container.style.maxHeight = `${maxHeightPx}px`;
      container.style.setProperty('--beach-terminal-max-height', `${maxHeightPx}px`);
      const current = store.getSnapshot();
      log('resize', {
        containerHeight: containerRect.height,
        viewportHeight,
        lineHeight,
        measuredRows: measured,
        windowRows,
        viewportRows,
        lastSent: lastSentViewportRows.current,
        baseRow: current.baseRow,
        totalRows: current.rows.length,
        followTail: current.followTail,
      });
      store.setViewport(current.viewportTop, viewportRows);
      if (suppressNextResizeRef.current) {
        suppressNextResizeRef.current = false;
        return;
      }
      // Only send resize if the viewport size actually changed
      if (subscriptionRef.current !== null && transportRef.current && viewportRows !== lastSentViewportRows.current) {
        transportRef.current.send({ type: 'resize', cols: current.cols, rows: viewportRows });
        lastSentViewportRows.current = viewportRows;
      }
    });
    // Observe the wrapper, not the scroll container
    observer.observe(wrapper);
    return () => observer.disconnect();
  }, [effectiveLineHeight, lineHeight, store]);

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
    if (!element || !snapshot.followTail) {
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
        element.scrollTop = desired;
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
  }, [snapshot.followTail, snapshot.baseRow, snapshot.rows.length, lastAbsolute, lineHeight, topPadding, bottomPadding, lines.length, effectiveLineHeight]);

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
      const overlayUpdate = predictionUxRef.current.recordSend(seq, timestampMs, predicts && predictionApplied);
      if (overlayUpdate) {
        setPredictionOverlay(overlayUpdate);
      }
    }
  }, [store]);

  const enqueueInput = useCallback((data: Uint8Array, predict: boolean) => {
    pendingInputRef.current.push({ data, predict });
    if (flushTimerRef.current === null) {
      // Micro-batch on the next task to allow multiple key events in one frame.
      flushTimerRef.current = window.setTimeout(flushPendingInput, 2);
    }
  }, [flushPendingInput]);

  const handleKeyDown: React.KeyboardEventHandler<HTMLDivElement> = (event) => {
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

  const containerStyle: CSSProperties & { '--beach-terminal-line-height': string } = {
    fontFamily,
    fontSize,
    lineHeight: `${lineHeight}px`,
    letterSpacing: '0.01em',
    fontVariantLigatures: 'none',
    // Prevent Chrome scroll anchoring from fighting spacer adjustments during
    // zoom/resize, which can cause off-screen rows to jump into view.
    overflowAnchor: 'none',
    '--beach-terminal-line-height': `${lineHeight}px`,
  };

  const handleMatchPtyViewport = useCallback(() => {
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
    log('match_host_viewport', {
      targetRows: clampedRows,
      targetCols,
      subscription,
    });
    try {
      transport.send({ type: 'resize', cols: targetCols, rows: clampedRows });
    } catch (err) {
      if (IS_DEV) {
        console.warn('[beach-surfer] match_host_viewport send failed', err);
      }
    }
  }, [log, store]);
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
  const matchButtonDisabled = !hasPtyResizeTarget || status !== 'connected';
  const matchButtonTitle = hasPtyResizeTarget
    ? `Match PTY size ${fallbackColsForLabel}×${ptyViewportRows}`
    : 'Host PTY size unavailable yet';
  const matchButtonAriaLabel = hasPtyResizeTarget
    ? `Resize to host PTY size ${fallbackColsForLabel} by ${ptyViewportRows}`
    : 'Resize to host PTY size (unavailable)';
  const matchButtonClass = cn(
    'inline-flex h-3.5 w-3.5 items-center justify-center rounded-full transition focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-white/40',
    matchButtonDisabled
      ? 'cursor-not-allowed border border-[#1b2638] bg-[#101724] text-[#4c5d7f] opacity-60'
      : 'border border-[#254f8c] bg-[#2d60aa] text-[#d7e4ff] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.18)] hover:bg-[#346bc0]',
  );

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
      <JoinStatusOverlay state={joinState} message={joinMessage} isFullscreen={isFullscreen} />
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
          {renderStatus()}
        </footer>
      ) : null}
    </div>
  );

  function handleScroll(event: UIEvent<HTMLDivElement>): void {
    const element = event.currentTarget;
    const pixelsPerRow = Math.max(1, effectiveLineHeight);
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
    const atBottom = shouldReenableFollowTail(remainingPixels, pixelsPerRow);
    const nearBottom = remainingPixels <= pixelsPerRow * 2;
    const previousFollowTail = snapshot.followTail;
    store.setFollowTail(atBottom);
    const nextSnapshot = store.getSnapshot();
    trace('scroll tail decision', {
      previousFollowTail,
      requestedFollowTail: atBottom,
      appliedFollowTail: nextSnapshot.followTail,
      nearBottom,
      remainingPixels,
      lineHeight,
      measuredLineHeight: pixelsPerRow,
      viewportRows,
      measuredRows,
      approxRow,
      baseRow: snapshot.baseRow,
      viewportTop: nextSnapshot.viewportTop,
      viewportHeight: nextSnapshot.viewportHeight,
      totalRows,
      firstAbsolute,
      lastAbsolute,
    });
    logScrollDiagnostics(
      element,
      remainingPixels,
      viewportRows,
      atBottom,
      nextSnapshot,
      lines,
      firstAbsolute,
      lastAbsolute,
    );
    backfillController.maybeRequest(nextSnapshot, nearBottom);
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

    if (!handshakeReadyRef.current) {
      enterWaitingState();
    }
  }

  function handleHostFrame(frame: HostFrame): void {
    backfillController.handleFrame(frame);
    switch (frame.type) {
      case 'hello':
        trace('frame hello', frame);
        if (ptyViewportRowsRef.current !== null) {
          ptyViewportRowsRef.current = null;
          setPtyViewportRows((prev) => (prev === null ? prev : null));
        }
        if (ptyColsRef.current !== null) {
          ptyColsRef.current = null;
          setPtyCols((prev) => (prev === null ? prev : null));
        }
        store.reset();
        subscriptionRef.current = frame.subscription;
        inputSeqRef.current = 0;
        store.setCursorSupport(Boolean(frame.features & FEATURE_CURSOR_SYNC));
        {
          const overlayReset = predictionUxRef.current.reset(now());
          if (overlayReset) {
            setPredictionOverlay(overlayReset);
          }
        }
        summarizeSnapshot(store);
        handshakeReadyRef.current = true;
        enterApprovedState(joinStateRef.current === 'approved' ? joinMessage ?? undefined : undefined);
        markConnectionTrace('beach_terminal:hello_received', {
          subscription: frame.subscription,
        });
        finishConnectionTrace('success', {
          remotePeerId: remotePeerId ?? connectionRef.current?.remotePeerId ?? null,
          subscription: frame.subscription,
        });
        break;
      case 'grid':
        trace('frame grid', frame);
        {
          const nextCols = Math.max(1, frame.cols);
          ptyColsRef.current = nextCols;
          setPtyCols((prev) => (prev === nextCols ? prev : nextCols));
        }
        if (typeof frame.viewportRows === 'number' && frame.viewportRows > 0) {
          const clampedRows = Math.max(1, Math.min(frame.viewportRows, MAX_VIEWPORT_ROWS));
          ptyViewportRowsRef.current = clampedRows;
          setPtyViewportRows((prev) => (prev === clampedRows ? prev : clampedRows));
        }
        store.setBaseRow(frame.baseRow);
        store.setGridSize(frame.historyRows, frame.cols);
        store.setFollowTail(false);
        {
          const historyEnd = frame.baseRow + frame.historyRows;
          const deviceViewport = Math.max(
            1,
            Math.min(lastMeasuredViewportRows.current, MAX_VIEWPORT_ROWS),
          );
          const viewportTop = frame.baseRow;
          store.setViewport(viewportTop, deviceViewport);
          if (lastSentViewportRows.current === 0) {
            lastSentViewportRows.current = deviceViewport;
          }
          suppressNextResizeRef.current = true;
          log('grid frame', {
            baseRow: frame.baseRow,
            historyRows: frame.historyRows,
            cols: frame.cols,
            serverViewport: frame.viewportRows ?? null,
            deviceViewport,
            viewportTop,
            historyEnd,
          });
        }
        {
          const overlayReset = predictionUxRef.current.reset(now());
          if (overlayReset) {
            setPredictionOverlay(overlayReset);
          }
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
        const current = store.getSnapshot();
        backfillController.maybeRequest(current, current.followTail);
        break;
      }
      case 'snapshot_complete':
        break;
      case 'input_ack': {
        const timestamp = now();
        predictiveLog('server_frame', { frame: 'input_ack', seq: frame.seq });
        store.ackPrediction(frame.seq, timestamp);
        const overlayUpdate = predictionUxRef.current.recordAck(frame.seq, timestamp);
        if (overlayUpdate) {
          setPredictionOverlay(overlayUpdate);
        }
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
  limit: number,
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

  trace('buildLines result', {
    limit,
    followTail: snapshot.followTail,
    viewportTop: snapshot.viewportTop,
    viewportHeight: snapshot.viewportHeight,
    baseRow: snapshot.baseRow,
    rowKinds: rows.map((row) => row.kind),
    absolutes: rows.map((row) => row.absolute),
    lineKinds: lines.map((line) => line.kind),
    lineAbsolutes: lines.map((line) => line.absolute),
  });
  if (typeof console !== 'undefined') {
    console.info('[beach-trace][terminal][buildLines result]', {
      limit,
      followTail: snapshot.followTail,
      viewportTop: snapshot.viewportTop,
      viewportHeight: snapshot.viewportHeight,
      baseRow: snapshot.baseRow,
      rowKinds: rows.map((row) => row.kind),
      absolutes: rows.map((row) => row.absolute),
      lineKinds: lines.map((line) => line.kind),
      lineAbsolutes: lines.map((line) => line.absolute),
    });
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

export function shouldReenableFollowTail(remainingPixels: number, lineHeightPx: number): boolean {
  const tolerance = Math.max(1, Math.ceil(lineHeightPx * 2));
  return remainingPixels <= tolerance;
}

const DEFAULT_FOREGROUND = '#e2e8f0';
const DEFAULT_BACKGROUND = 'hsl(var(--terminal-screen))';
const NBSP = '\u00A0';

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
