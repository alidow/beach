import type { CSSProperties, UIEvent } from 'react';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { FEATURE_CURSOR_SYNC } from '../protocol/types';
import type { HostFrame } from '../protocol/types';
import { createTerminalStore, useTerminalSnapshot } from '../terminal/useTerminalState';
import { connectBrowserTransport, type BrowserTransportConnection } from '../terminal/connect';
import type { CellState, TerminalGridSnapshot, TerminalGridStore } from '../terminal/gridStore';
import { encodeKeyEvent } from '../terminal/keymap';
import type { TerminalTransport } from '../transport/terminalTransport';
import { BackfillController } from '../terminal/backfillController';
import { cn } from '../lib/utils';
import type { ServerMessage } from '../transport/signaling';

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

declare global {
  interface Window {
    __BEACH_TRACE?: boolean;
  }
}

function trace(...args: unknown[]): void {
  if (typeof window !== 'undefined' && window.__BEACH_TRACE) {
    console.debug('[beach-trace][terminal]', ...args);
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
    cursor: { row: snapshot.cursorRow, col: snapshot.cursorCol, seq: snapshot.cursorSeq },
    predictedCursor: snapshot.predictedCursor,
    preview,
  });
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
    className,
    fontFamily = "'SFMono-Regular', 'Menlo', 'Consolas', monospace",
    fontSize = 14,
    showStatusBar = true,
    isFullscreen = false,
    onToggleFullscreen,
  } = props;

  const store = useMemo(() => providedStore ?? createTerminalStore(), [providedStore]);
  if (import.meta.env.DEV) {
    (window as any).beachStore = store;
  }
  const snapshot = useTerminalSnapshot(store);
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const headerRef = useRef<HTMLDivElement | null>(null);
  const transportRef = useRef<TerminalTransport | null>(providedTransport ?? null);
  const connectionRef = useRef<BrowserTransportConnection | null>(null);
  const subscriptionRef = useRef<number | null>(null);
  const inputSeqRef = useRef(0);
  const lastSentViewportRows = useRef<number>(0);
  const lastMeasuredViewportRows = useRef<number>(24);
  const suppressNextResizeRef = useRef<boolean>(false);
  const [status, setStatus] = useState<TerminalStatus>(
    providedTransport ? 'connected' : 'idle',
  );
  const [error, setError] = useState<Error | null>(null);
  const [showIdlePlaceholder, setShowIdlePlaceholder] = useState(true);
  const [headerHeight, setHeaderHeight] = useState<number>(0);
  const [activeConnection, setActiveConnection] = useState<BrowserTransportConnection | null>(null);
  const [peerId, setPeerId] = useState<string | null>(null);
  const [remotePeerId, setRemotePeerId] = useState<string | null>(null);
  const [joinState, setJoinState] = useState<JoinOverlayState>('idle');
  const joinStateRef = useRef<JoinOverlayState>('idle');
  const [joinMessage, setJoinMessage] = useState<string | null>(null);
  const joinTimersRef = useRef<{ short?: number; long?: number; hide?: number }>({});
  const peerIdRef = useRef<string | null>(null);
  const handshakeReadyRef = useRef(false);
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
  const log = useCallback((message: string, detail?: Record<string, unknown>) => {
    if (!import.meta.env.DEV) {
      return;
    }
    const current = peerIdRef.current;
    const prefix = current ? `[beach-web:${current.slice(0, 8)}]` : '[beach-web]';
    if (detail) {
      console.debug(`${prefix} ${message}`, detail);
    } else {
      console.debug(`${prefix} ${message}`);
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
    } else if (status === 'idle') {
      setShowIdlePlaceholder(true);
    }
  }, [status, onStatusChange]);
  const lines = useMemo(() => buildLines(snapshot, 600), [snapshot]);
  if (import.meta.env.DEV && typeof window !== 'undefined' && window.__BEACH_TRACE) {
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
  if (import.meta.env.DEV) {
    (window as any).beachLines = lines;
  }
  const lineHeight = computeLineHeight(fontSize);
  const [measuredLineHeight, setMeasuredLineHeight] = useState<number>(lineHeight);
  const effectiveLineHeight = measuredLineHeight > 0 ? measuredLineHeight : lineHeight;
  const totalRows = snapshot.rows.length;
  const firstAbsolute = lines.length > 0 ? lines[0]!.absolute : snapshot.baseRow;
  const lastAbsolute = lines.length > 0 ? lines[lines.length - 1]!.absolute : firstAbsolute;
  const topPaddingRows = Math.max(0, firstAbsolute - snapshot.baseRow);
  const bottomPaddingRows = Math.max(0, snapshot.baseRow + totalRows - (lastAbsolute + 1));
  const topPadding = topPaddingRows * effectiveLineHeight;
  const bottomPadding = bottomPaddingRows * effectiveLineHeight;
  const backfillController = useMemo(
    () => new BackfillController(store, (frame) => transportRef.current?.send(frame)),
    [store],
  );

  useLayoutEffect(() => {
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
    const handleResize = () => {
      if (raf !== -1) {
        window.cancelAnimationFrame(raf);
      }
      raf = window.requestAnimationFrame(measure);
    };
    window.addEventListener('resize', handleResize);
    return () => {
      window.removeEventListener('resize', handleResize);
      if (raf !== -1) {
        window.cancelAnimationFrame(raf);
      }
    };
  }, []);

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
    let cancelled = false;
    setStatus('connecting');
    setJoinState('connecting');
    setJoinMessage(JOIN_CONNECTING_MESSAGE);
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

        const connection = await connectBrowserTransport({
          sessionId,
          baseUrl,
          passcode,
          logger: webrtcLogger,
          clientLabel: queryLabel,
        });
        if (cancelled) {
          connection.close();
          return;
        }
        connectionRef.current = connection;
        transportRef.current = connection.transport;
        setActiveConnection(connection);
        setPeerId(connection.signaling.peerId);
        setRemotePeerId(connection.remotePeerId ?? null);
        bindTransport(connection.transport);
        setStatus('connected');
      } catch (err) {
        if (cancelled) {
          return;
        }
        setError(err instanceof Error ? err : new Error(String(err)));
        setStatus('error');
      }
    })();

    return () => {
      cancelled = true;
      if (connectionRef.current) {
        connectionRef.current.close();
      }
      connectionRef.current = null;
      transportRef.current = null;
      setActiveConnection(null);
      setPeerId(null);
      setRemotePeerId(null);
      handshakeReadyRef.current = false;
      clearJoinTimers();
      setJoinState('idle');
      setJoinMessage(null);
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
      const viewportEstimate = Math.max(1, Math.floor(element.clientHeight / rowHeight));
      let desired = target;
      const lastContentAbsolute = findLastContentAbsolute(snapshot);
      if (
        lastContentAbsolute !== null &&
        lastContentAbsolute >= snapshot.baseRow
      ) {
        const totalContentRows = lastContentAbsolute - snapshot.baseRow + 1;
        if (totalContentRows <= viewportEstimate) {
          desired = 0;
        }
      }
      if (import.meta.env.DEV && typeof window !== 'undefined' && window.__BEACH_TRACE) {
        console.debug('[beach-trace][terminal] autoscroll', {
          before: element.scrollTop,
          target,
          desired,
          scrollHeight: element.scrollHeight,
          clientHeight: element.clientHeight,
        });
      }
      element.scrollTop = desired;
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

  const handleKeyDown: React.KeyboardEventHandler<HTMLDivElement> = (event) => {
    const transport = transportRef.current;
    if (!transport) {
      return;
    }
    const payload = encodeKeyEvent(event.nativeEvent);
    if (!payload || payload.length === 0) {
      return;
    }
    if (subscriptionRef.current === null) {
      return;
    }
    event.preventDefault();
    const seq = ++inputSeqRef.current;
    transport.send({ type: 'input', seq, data: payload });
    store.registerPrediction(seq, payload);
  };

  // Store original position for smooth fullscreen animation
  const [wrapperStyle, setWrapperStyle] = useState<CSSProperties>({});

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
    'relative flex h-full min-h-0 flex-col overflow-hidden',
    'rounded-[22px] border border-[#0f131a] bg-[#090d14]/95 shadow-[0_45px_120px_-70px_rgba(10,26,55,0.85)]',
    isFullscreen && 'z-50 rounded-none',
    className,
  );
  const containerClasses = cn(
    'beach-terminal relative flex-1 min-h-0 overflow-y-auto overflow-x-auto whitespace-pre font-mono text-[13px] leading-[1.42] text-[#d5d9e0]',
    'bg-[#1b1f2a] px-6 py-5 shadow-[inset_0_0_0_1px_rgba(255,255,255,0.04),inset_0_22px_45px_-25px_rgba(8,10,20,0.82)]',
  );

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

  return (
    <div ref={wrapperRef} className={wrapperClasses}>
      <div className="pointer-events-none absolute inset-0">
        <div className="absolute inset-x-0 top-0 h-28 bg-gradient-to-b from-white/12 via-white/0 to-transparent opacity-20" aria-hidden />
        <div className="absolute inset-0 rounded-[22px] ring-1 ring-[#1f2736]/60" aria-hidden />
      </div>
      <JoinStatusOverlay state={joinState} message={joinMessage} />
      <header
        ref={headerRef}
        className="relative z-10 flex items-center justify-between gap-4 bg-[#111925]/95 px-6 py-3 text-[11px] font-medium uppercase tracking-[0.36em] text-[#9aa4bc]"
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
          <span className="text-[10px] font-semibold uppercase tracking-[0.5em] text-[#c0cada]">{sessionTitle}</span>
        </div>
        <div className="flex items-center gap-2 text-[10px]">
          <span className="inline-flex items-center gap-2 rounded-full border border-white/10 px-3 py-1 text-[10px] font-semibold uppercase tracking-[0.32em] text-[#c9d2e5]">
            <span className="size-1.5 rounded-full" style={{ backgroundColor: statusColor }} aria-hidden />
            {renderStatus()}
          </span>
        </div>
      </header>
      <div
        ref={containerRef}
        className={containerClasses}
        tabIndex={0}
        onKeyDown={handleKeyDown}
        onScroll={handleScroll}
        style={{
          fontFamily,
          fontSize,
          lineHeight: `${lineHeight}px`,
          letterSpacing: '0.01em',
          fontVariantLigatures: 'none',
          '--beach-terminal-line-height': `${lineHeight}px`,
        }}
      >
        {showIdlePlaceholder ? (
          <IdlePlaceholder
            topOffset={headerHeight}
            onConnectNotice={() => setShowIdlePlaceholder(false)}
            status={status}
          />
        ) : null}
        <div style={{ height: topPadding }} aria-hidden="true" />
        {lines.map((line) => (
          <LineRow key={line.absolute} line={line} styles={snapshot.styles} />
        ))}
        <div style={{ height: bottomPadding }} aria-hidden="true" />
      </div>
      {showStatusBar ? (
        <footer className="flex items-center gap-2 px-6 pb-3 text-xs text-[hsl(var(--muted-foreground))]">
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
    const atBottom = shouldReenableFollowTail(remainingPixels);
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
      return `Error: ${error.message}`;
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
    transport.addEventListener('open', () => {
      if (!handshakeReadyRef.current) {
        enterWaitingState();
      }
    });
    transport.addEventListener(
      'close',
      () => {
        const remote = remotePeerId ?? connectionRef.current?.remotePeerId ?? null;
        log('transport closed', { remotePeerId: remote });
        handshakeReadyRef.current = false;
        if (subscriptionRef.current === null && joinStateRef.current !== 'denied') {
          enterDisconnectedState();
        }
        subscriptionRef.current = null;
        setStatus('closed');
      },
      { once: true },
    );
    transport.addEventListener('error', (event) => {
      const err = (event as any).error ?? new Error('transport error');
      const remote = remotePeerId ?? connectionRef.current?.remotePeerId ?? null;
      log('transport error', { message: err.message, remotePeerId: remote });
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
        store.reset();
        subscriptionRef.current = frame.subscription;
        inputSeqRef.current = 0;
        store.setCursorSupport(Boolean(frame.features & FEATURE_CURSOR_SYNC));
        summarizeSnapshot(store);
        handshakeReadyRef.current = true;
        enterApprovedState(joinStateRef.current === 'approved' ? joinMessage ?? undefined : undefined);
        break;
      case 'grid':
        trace('frame grid', frame);
        store.setBaseRow(frame.baseRow);
        store.setGridSize(frame.historyRows, frame.cols);
        {
          const historyEnd = frame.baseRow + frame.historyRows;
          const deviceViewport = Math.max(
            1,
            Math.min(lastMeasuredViewportRows.current, MAX_VIEWPORT_ROWS),
          );
          const viewportTop = Math.max(historyEnd - deviceViewport, frame.baseRow);
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
          });
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
        store.applyUpdates(frame.updates, {
          authoritative,
          origin: frame.type,
          cursor: frame.cursor ?? null,
        });
        if (frame.type === 'history_backfill') {
          backfillController.finalizeHistoryBackfill(frame);
        }
        summarizeSnapshot(store);
        if (!frame.hasMore && frame.type === 'snapshot') {
          store.setFollowTail(true);
        }
        const current = store.getSnapshot();
        backfillController.maybeRequest(current, current.followTail);
        break;
      }
      case 'snapshot_complete':
        break;
      case 'input_ack':
        store.clearPrediction(frame.seq);
        break;
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
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    const slot = rows[index];
    if (!slot) {
      continue;
    }
    if (slot.kind !== 'loaded') {
      return slot.absolute;
    }
    if (rowHasVisibleContent(slot.cells)) {
      return slot.absolute;
    }
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

interface RenderLine {
  absolute: number;
  kind: 'loaded' | 'pending' | 'missing';
  cells?: RenderCell[];
  cursorCol?: number | null;
  predictedCursorCol?: number | null;
}

export function buildLines(snapshot: TerminalGridSnapshot, limit: number): RenderLine[] {
  const rows = snapshot.visibleRows(limit);
  if (rows.length === 0) {
    return [];
  }

  const placeholderWidth = Math.max(1, snapshot.cols || 80);
  const lines: RenderLine[] = [];

  for (const row of rows) {
    if (row.kind === 'loaded') {
      const cells = row.cells.map((cell) => ({
        char: cell.char ?? ' ',
        styleId: cell.styleId ?? 0,
      }));
      const predictions = snapshot.predictionsForRow(row.absolute);
      if (predictions.length > 0) {
        for (const { col, cell: prediction } of predictions) {
          while (cells.length <= col) {
            cells.push({ char: ' ', styleId: 0 });
          }
          const existing = cells[col];
          cells[col] = {
            char: prediction.char ?? ' ',
            styleId: existing?.styleId ?? 0,
            predicted: true,
          };
        }
      }
      let cursorCol: number | null = null;
      if (snapshot.cursorVisible && snapshot.cursorRow === row.absolute && snapshot.cursorCol !== null) {
        const raw = Math.floor(Math.max(snapshot.cursorCol, 0));
        cursorCol = Number.isFinite(raw) ? raw : null;
      }
      let predictedCursorCol: number | null = null;
      if (
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

  return lines;
}

function JoinStatusOverlay({ state, message }: { state: JoinOverlayState; message: string | null }): JSX.Element | null {
  if (state === 'idle') {
    return null;
  }
  const text = message ?? JOIN_WAIT_DEFAULT;
  const showSpinner = state === 'connecting' || state === 'waiting';
  const badgeText = state === 'approved' ? 'OK' : state === 'denied' ? 'NO' : 'OFF';

  return (
    <div className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center bg-[#05070b]/80 backdrop-blur-sm">
      <div className="pointer-events-auto flex w-[min(420px,90%)] flex-col items-center gap-3 rounded-lg border border-white/10 bg-[#111827]/95 px-6 py-5 text-center text-sm text-slate-200 shadow-2xl">
        {showSpinner ? (
          <div className="h-8 w-8 animate-spin rounded-full border-2 border-white/40 border-t-transparent" />
        ) : (
          <div className="flex h-8 w-8 items-center justify-center rounded-full border border-white/30 text-xs font-semibold uppercase tracking-wide text-white/80">
            {badgeText}
          </div>
        )}
        <div className="font-mono text-[13px] text-slate-100">{text}</div>
        {state === 'waiting' ? (
          <p className="text-xs text-slate-400">Your typing stays local until the host approves.</p>
        ) : null}
        {state === 'connecting' ? (
          <p className="text-xs text-slate-400">Negotiating WebRTC connection...</p>
        ) : null}
        {state === 'approved' ? (
          <p className="text-xs text-emerald-300">Syncing terminal...</p>
        ) : null}
        {state === 'denied' ? (
          <p className="text-xs text-rose-300">You can close this window.</p>
        ) : null}
        {state === 'disconnected' ? (
          <p className="text-xs text-amber-300">Connection closed before approval.</p>
        ) : null}
      </div>
    </div>
  );
}

function LineRow({ line, styles }: { line: RenderLine; styles: Map<number, StyleDefinition> }): JSX.Element {
  if (!line.cells || line.kind !== 'loaded') {
    const text = line.cells?.map((cell) => cell.char).join('') ?? '';
    const className = cn('xterm-row', line.kind === 'pending' ? 'opacity-60' : undefined);
    return <div className={className}>{text}</div>;
  }

  const cursorCol = line.cursorCol ?? null;
  const predictedCursorCol = line.predictedCursorCol ?? null;
  const baseStyleDef = styles.get(0) ?? { id: 0, fg: 0, bg: 0, attrs: 0 };

  return (
    <div className="xterm-row">
      {line.cells.map((cell, index) => {
        const styleDef = styles.get(cell.styleId);
        const isCursor = cursorCol !== null && cursorCol === index;
        let style = styleDef ? styleFromDefinition(styleDef, isCursor) : undefined;
        const isPredictedCursor = predictedCursorCol !== null && predictedCursorCol === index;
        const predicted = cell.predicted === true || (isPredictedCursor && !isCursor);
        if (predicted) {
          const merged: CSSProperties = { ...(style ?? {}) };
          merged.textDecoration = appendTextDecoration(merged.textDecoration, 'underline');
          const existingOpacity = merged.opacity !== undefined ? Number(merged.opacity) : undefined;
          merged.opacity = existingOpacity !== undefined ? existingOpacity * 0.75 : 0.75;
          style = merged;
        }
        const char = cell.char === ' ' ? ' ' : cell.char;
        return (
          <span
            key={index}
            style={style}
            data-predicted={predicted || undefined}
            data-predicted-cursor={isPredictedCursor || undefined}
          >
            {char}
          </span>
        );
      })}
      {cursorCol !== null && cursorCol >= line.cells.length ? (
        <span key="cursor" style={styleFromDefinition(baseStyleDef, true)}> </span>
      ) : null}
      {predictedCursorCol !== null && predictedCursorCol >= line.cells.length ? (
        <span
          key="predicted-cursor"
          style={{
            ...styleFromDefinition(baseStyleDef, false),
            textDecoration: appendTextDecoration(undefined, 'underline'),
            opacity: 0.75,
          }}
          data-predicted
          data-predicted-cursor
        >
           
        </span>
      ) : null}
    </div>
  );
}

function computeLineHeight(fontSize: number): number {
  return Math.round(fontSize * 1.4);
}

export function shouldReenableFollowTail(remainingPixels: number): boolean {
  return remainingPixels <= 1;
}

const DEFAULT_FOREGROUND = '#e2e8f0';
const DEFAULT_BACKGROUND = '#020617';

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

function appendTextDecoration(existing: string | undefined, value: string): string {
  if (!existing) {
    return value;
  }
  if (existing.includes(value)) {
    return existing;
  }
  return `${existing} ${value}`.trim();
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
  topOffset,
}: {
  onConnectNotice: () => void;
  status: TerminalStatus;
  topOffset: number;
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
