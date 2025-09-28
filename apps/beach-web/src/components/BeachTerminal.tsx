import type { CSSProperties, UIEvent } from 'react';
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
  const totalRows = snapshot.rows.length;
  const firstAbsolute = lines.length > 0 ? lines[0]!.absolute : snapshot.baseRow;
  const lastAbsolute = lines.length > 0 ? lines[lines.length - 1]!.absolute : firstAbsolute;
  const topPaddingRows = Math.max(0, firstAbsolute - snapshot.baseRow);
  const bottomPaddingRows = Math.max(0, snapshot.baseRow + totalRows - (lastAbsolute + 1));
  const topPadding = topPaddingRows * lineHeight;
  const bottomPadding = bottomPaddingRows * lineHeight;
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

  useEffect(() => {
    const element = containerRef.current;
    if (!element || !snapshot.followTail) {
      return;
    }
    const target = element.scrollHeight - element.clientHeight;
    if (target < 0) {
      return;
    }
    if (Math.abs(element.scrollTop - target) > 1) {
      element.scrollTop = target;
    }
  }, [snapshot.followTail, snapshot.baseRow, snapshot.rows.length, lastAbsolute, lineHeight, topPadding, bottomPadding]);

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

  const wrapperClasses = ['flex flex-col h-full min-h-0 gap-3', className]
    .filter(Boolean)
    .join(' ');
  const containerClasses = 'beach-terminal flex-1 min-h-0 overflow-y-auto overflow-x-auto whitespace-pre font-mono text-sm text-slate-100 bg-slate-950/95 border border-slate-800/60 rounded-xl shadow-inner px-4 py-3';

  return (
    <div className={wrapperClasses}>
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
          minHeight: lineHeight * Math.max(1, minimumRows),
          height: '100%',
          maxHeight: '100%',
        }}
      >
        <div style={{ height: topPadding }} aria-hidden="true" />
        {lines.map((line) => (
          <LineRow key={line.absolute} line={line} styles={snapshot.styles} />
        ))}
        <div style={{ height: bottomPadding }} aria-hidden="true" />
      </div>
      <footer className="text-xs text-slate-400">{renderStatus()}</footer>
    </div>
  );

  function handleScroll(event: UIEvent<HTMLDivElement>): void {
    const element = event.currentTarget;
    const approxRow = Math.max(
      snapshot.baseRow,
      snapshot.baseRow + Math.floor(element.scrollTop / lineHeight),
    );
    const measuredRows = Math.max(1, Math.floor(element.clientHeight / lineHeight));
    const viewportRows = Math.max(minimumRows, measuredRows);
    const maxTop = Math.max(snapshot.baseRow, snapshot.baseRow + totalRows - viewportRows);
    const clampedTop = Math.min(approxRow, maxTop);

    store.setViewport(clampedTop, viewportRows);

    const nearBottom = element.scrollHeight - (element.scrollTop + element.clientHeight) < lineHeight * 2;
    store.setFollowTail(nearBottom);
    if (!nearBottom) {
      backfillController.maybeRequest(store.getSnapshot(), false);
    }
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

interface RenderCell {
  char: string;
  styleId: number;
}

interface RenderLine {
  absolute: number;
  kind: 'loaded' | 'pending' | 'missing';
  cells?: RenderCell[];
  cursorCol?: number | null;
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
      let cursorCol: number | null = null;
      if (snapshot.cursorRow === row.absolute && snapshot.cursorCol !== null && cells.length > 0) {
        const maxIndex = Math.max(cells.length - 1, 0);
        const clamped = Math.min(Math.max(snapshot.cursorCol, 0), maxIndex);
        cursorCol = Number.isFinite(clamped) ? clamped : null;
      }
      lines.push({ absolute: row.absolute, kind: 'loaded', cells, cursorCol });
      continue;
    }
    const fillChar = row.kind === 'pending' ? '·' : ' ';
    const width = row.kind === 'pending' ? placeholderWidth : placeholderWidth;
    const cells = Array.from({ length: width }, () => ({ char: fillChar, styleId: 0 }));
    lines.push({ absolute: row.absolute, kind: row.kind, cells });
  }

  return lines;
}

function LineRow({ line, styles }: { line: RenderLine; styles: Map<number, StyleDefinition> }): JSX.Element {
  if (!line.cells || line.kind !== 'loaded') {
    const text = line.cells?.map((cell) => cell.char).join('') ?? '';
    const className = line.kind === 'pending' ? 'opacity-60' : undefined;
    return <div className={className}>{text}</div>;
  }

  return (
    <div>
      {line.cells.map((cell, index) => {
        const styleDef = styles.get(cell.styleId);
        const isCursor = line.cursorCol !== null && line.cursorCol === index;
        const style = styleDef ? styleFromDefinition(styleDef, isCursor) : undefined;
        const char = cell.char === ' ' ? ' ' : cell.char;
        return (
          <span key={index} style={style}>
            {char}
          </span>
        );
      })}
    </div>
  );
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
