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
  if (import.meta.env.DEV) {
    (window as any).beachLines = lines;
  }
  const [minimumRows, setMinimumRows] = useState(24);
  const lineHeight = computeLineHeight(fontSize);
  const backfillController = useMemo(
    () => new BackfillController(store, (frame) => transportRef.current?.send(frame)),
    [store],
  );

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
      const measured = Math.max(1, Math.floor(entry.contentRect.height / lineHeight));
      const viewportRows = Math.max(minimumRows, measured);
      if (import.meta.env.DEV) {
        console.debug('[beach-web] resize', {
          height: entry.contentRect.height,
          lineHeight,
          measuredRows: measured,
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
  }, [lineHeight, minimumRows, store]);

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

  const containerClasses = ['beach-terminal flex-1 overflow-auto whitespace-pre font-mono text-sm text-slate-100 bg-slate-950/95 border border-slate-800/60 rounded-xl shadow-inner px-4 py-3', className]
    .filter(Boolean)
    .join(' ');

  return (
    <div className="flex flex-col gap-3">
      <div
        ref={containerRef}
        className={containerClasses}
        tabIndex={0}
        onKeyDown={handleKeyDown}
        style={{
          fontFamily,
          fontSize,
          lineHeight: `${lineHeight}px`,
          minHeight: lineHeight * Math.max(1, minimumRows),
        }}
      >
        {lines.map((line) => (
          <div
            key={line.absolute}
            className={line.kind === 'pending' ? 'opacity-60' : undefined}
          >
            {line.text}
          </div>
        ))}
      </div>
      <footer className="text-xs text-slate-400">{renderStatus()}</footer>
    </div>
  );

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
        setMinimumRows(frame.viewportRows);
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
  const rows = snapshot.visibleRows(limit);
  if (rows.length === 0) {
    return [];
  }

  const placeholderWidth = Math.max(1, snapshot.cols || 80);
  const lines: RenderLine[] = [];

  for (const row of rows) {
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
