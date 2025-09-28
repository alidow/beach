import { useEffect, useMemo, useRef, useState } from 'react';
import type { ClientFrame, HostFrame } from '../protocol/types';
import { createTerminalStore, useTerminalSnapshot } from '../terminal/useTerminalState';
import { connectBrowserTransport, type BrowserTransportConnection } from '../terminal/connect';
import type { TerminalGridSnapshot, TerminalGridStore } from '../terminal/gridStore';
import { encodeKeyEvent } from '../terminal/keymap';
import type { TerminalTransport } from '../transport/terminalTransport';
import { BackfillController } from '../terminal/backfillController';

export interface BeachTerminalProps {
  sessionId?: string;
  baseUrl?: string;
  passcode?: string;
  autoConnect?: boolean;
  store?: TerminalGridStore;
  transport?: TerminalTransport;
  className?: string;
  style?: React.CSSProperties;
  fontFamily?: string;
  fontSize?: number;
}

export function BeachTerminal(props: BeachTerminalProps): JSX.Element {
  const {
    sessionId,
    baseUrl,
    passcode,
    autoConnect = false,
    transport: providedTransport,
    store: providedStore,
    className,
    style,
    fontFamily = "'SFMono-Regular', 'Menlo', 'Consolas', monospace",
    fontSize = 14,
  } = props;

  const store = useMemo(() => providedStore ?? createTerminalStore(), [providedStore]);
  if (import.meta.env.DEV) {
    (window as any).beachStore = store;
  }
  const snapshot = useTerminalSnapshot(store);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const transportRef = useRef<TerminalTransport | null>(providedTransport ?? null);
  const connectionRef = useRef<BrowserTransportConnection | null>(null);
  const subscriptionRef = useRef<number | null>(null);
  const inputSeqRef = useRef(0);
  const [status, setStatus] = useState<'idle' | 'connecting' | 'connected' | 'error' | 'closed'>(
    providedTransport ? 'connected' : 'idle',
  );
  const [error, setError] = useState<Error | null>(null);
  const lines = useMemo(() => buildLines(snapshot, 600), [snapshot]);
  const linesRef = useRef<RenderLine[]>(lines);
  const backfillController = useMemo(
    () => new BackfillController(store, (frame) => transportRef.current?.send(frame)),
    [store],
  );

  useEffect(() => {
    linesRef.current = lines;
  }, [lines]);

  useEffect(() => {
    transportRef.current = providedTransport ?? null;
    if (providedTransport) {
      bindTransport(providedTransport);
      setStatus('connected');
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [providedTransport]);

  useEffect(() => {
    if (!autoConnect || transportRef.current || !sessionId || !baseUrl) {
      return;
    }
    let cancelled = false;
    setStatus('connecting');
    (async () => {
      try {
        const connection = await connectBrowserTransport({
          sessionId,
          baseUrl,
          passcode,
          logger: (message) => console.log('[beach-web]', message),
        });
        if (cancelled) {
          connection.close();
          return;
        }
        connectionRef.current = connection;
        transportRef.current = connection.transport;
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
      connectionRef.current?.close();
      connectionRef.current = null;
      transportRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoConnect, sessionId, baseUrl, passcode]);

  useEffect(() => {
    if (!containerRef.current) {
      return;
    }
    const element = containerRef.current;
    const observer = new ResizeObserver((entries) => {
      const entry = entries[entries.length - 1];
      if (!entry) {
        return;
      }
      const lineHeight = computeLineHeight(fontSize);
      const viewportRows = Math.max(1, Math.floor(entry.contentRect.height / lineHeight));
      if (import.meta.env.DEV) {
        console.debug('[beach-web] resize', {
          height: entry.contentRect.height,
          lineHeight,
          viewportRows,
        });
      }
      const current = store.getSnapshot();
      store.setViewport(current.viewportTop, viewportRows);
      if (subscriptionRef.current !== null && transportRef.current) {
        sendResize(transportRef.current, current.cols, viewportRows);
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [fontSize, store]);

  useEffect(() => () => connectionRef.current?.close(), []);

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
    sendFrame(transport, { type: 'input', seq, data: payload });
  };

  const handleScroll: React.UIEventHandler<HTMLDivElement> = (event) => {
    const element = event.currentTarget;
    const lineHeight = computeLineHeight(fontSize);
    const linesSnapshot = linesRef.current;
    const firstAbsolute = linesSnapshot[0]?.absolute ?? snapshot.baseRow;
    const approxRow = firstAbsolute + Math.floor(element.scrollTop / lineHeight);
    const viewportRows = Math.max(1, Math.floor(element.clientHeight / lineHeight));
    store.setViewport(approxRow, viewportRows);

    const nearBottom =
      element.scrollHeight - (element.scrollTop + element.clientHeight) < lineHeight * 2;
    store.setFollowTail(nearBottom);
    backfillController.maybeRequest(store.getSnapshot(), nearBottom);
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0 }}>
      <div
        ref={containerRef}
        className={className}
        style={{
          ...style,
          flex: 1,
          overflow: 'auto',
          fontFamily,
          fontSize,
          lineHeight: `${computeLineHeight(fontSize)}px`,
          color: '#f8fafc',
          background: '#020617',
          borderRadius: 8,
          padding: '12px 16px',
          whiteSpace: 'pre',
          outline: 'none',
        }}
        tabIndex={0}
        onKeyDown={handleKeyDown}
        onScroll={handleScroll}
      >
        {lines.map((line) => (
          <div key={line.absolute} style={{ opacity: line.kind === 'pending' ? 0.4 : 1 }}>
            {line.text}
          </div>
        ))}
      </div>
      <footer style={{ marginTop: 8, fontSize: 12, opacity: 0.6, fontFamily }}>
        {status === 'error' && error
          ? `Error: ${error.message}`
          : status === 'connected'
            ? 'Connected'
            : status === 'connecting'
              ? 'Connecting…'
              : status === 'closed'
                ? 'Disconnected'
                : 'Idle'}
      </footer>
    </div>
  );

  function bindTransport(transport: TerminalTransport): void {
    const frameHandler = (event: Event) => {
      const frame = (event as CustomEvent<HostFrame>).detail;
      handleHostFrame(frame);
    };
    transport.addEventListener('frame', frameHandler as EventListener);
    transport.addEventListener('close', () => setStatus('closed'), { once: true });
    transport.addEventListener('error', (event) => {
      setError((event as any).error ?? new Error('transport error'));
      setStatus('error');
    });
  }

  function handleHostFrame(frame: HostFrame): void {
    backfillController.handleFrame(frame);
    switch (frame.type) {
      case 'hello':
        store.reset();
        subscriptionRef.current = frame.subscription;
        inputSeqRef.current = 0;
        break;
      case 'grid':
        store.setBaseRow(frame.baseRow);
        store.setGridSize(frame.historyRows, frame.cols);
        store.setViewport(0, frame.viewportRows);
        break;
      case 'snapshot':
      case 'delta':
      case 'history_backfill': {
        const authoritative = frame.type === 'snapshot' || frame.type === 'history_backfill';
        if (import.meta.env.DEV) {
          console.debug('[beach-web][updates]', frame.type, frame.updates);
        }
        store.applyUpdates(frame.updates, authoritative);
        if (import.meta.env.DEV) {
          const debugRows = store
            .getSnapshot()
            .rows.map((row) => ({
              absolute: row.absolute,
              kind: row.kind,
              text: row.kind === 'loaded' ? store.getRowText(row.absolute) : null,
            }));
          console.debug('[beach-web][rows]', debugRows);
        }
        if (!frame.hasMore && frame.type === 'snapshot') {
          store.setFollowTail(true);
        }
        const current = store.getSnapshot();
        if (!current.followTail) {
          backfillController.maybeRequest(current, current.followTail);
        }
        break;
      }
      case 'snapshot_complete':
      case 'input_ack':
      case 'heartbeat':
        break;
      case 'shutdown':
        setStatus('closed');
        break;
      default:
        break;
    }
  }
}

interface RenderLine {
  absolute: number;
  text: string;
  kind: 'loaded' | 'pending' | 'missing';
}

export function buildLines(snapshot: TerminalGridSnapshot, limit: number): RenderLine[] {
  const placeholderWidth = Math.max(1, snapshot.cols || 80);
  const rowsByAbsolute = new Map(snapshot.rows.map((row) => [row.absolute, row]));
  const highestLoaded = snapshot.rows.reduce<number | null>((acc, row) => {
    if (row.kind === 'loaded') {
      return acc === null || row.absolute > acc ? row.absolute : acc;
    }
    return acc;
  }, null);

  const availableRows = snapshot.rows.length;
  const fallbackHeight = availableRows ? Math.min(limit, availableRows) : 1;
  const viewportHeight = Math.max(1, Math.min(limit, snapshot.viewportHeight || fallbackHeight));

  let scrollTop: number;
  if (!snapshot.followTail) {
    scrollTop = Math.max(0, snapshot.viewportTop - snapshot.baseRow);
  } else if (highestLoaded !== null) {
    const desiredTop = highestLoaded - viewportHeight + 1;
    scrollTop = Math.max(0, desiredTop - snapshot.baseRow);
  } else {
    scrollTop = Math.max(0, snapshot.viewportTop - snapshot.baseRow);
  }

  const lines: RenderLine[] = [];
  for (let rowIdx = 0; rowIdx < viewportHeight && lines.length < limit; rowIdx += 1) {
    const absolute = snapshot.baseRow + scrollTop + rowIdx;
    const row = rowsByAbsolute.get(absolute);
    if (!row) {
      lines.push({ absolute, text: ' '.repeat(placeholderWidth), kind: 'missing' });
      continue;
    }
    if (row.kind === 'loaded') {
      const chars = row.cells.map((cell) => cell.char ?? ' ');
      while (chars.length && chars[chars.length - 1] === ' ') {
        chars.pop();
      }
      lines.push({ absolute: row.absolute, text: chars.join(''), kind: 'loaded' });
      continue;
    }
    if (row.kind === 'pending') {
      lines.push({ absolute: row.absolute, text: '·'.repeat(placeholderWidth), kind: 'pending' });
      continue;
    }
    lines.push({ absolute: row.absolute, text: ' '.repeat(placeholderWidth), kind: 'missing' });
  }

  if (snapshot.followTail) {
    while (lines.length > 0 && lines[lines.length - 1]?.kind !== 'loaded') {
      lines.pop();
    }
  }

  return lines;
}

function computeLineHeight(fontSize: number): number {
  return Math.round(fontSize * 1.4);
}

function sendFrame(transport: TerminalTransport, frame: ClientFrame): void {
  transport.send(frame);
}

function sendResize(transport: TerminalTransport, cols: number, rows: number): void {
  transport.send({ type: 'resize', cols, rows });
}
