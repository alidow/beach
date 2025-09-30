import type { CSSProperties, UIEvent } from 'react';
import { useEffect, useMemo, useRef, useState } from 'react';
import type { ClientFrame, HostFrame } from '../protocol/types';
import { createTerminalStore, useTerminalSnapshot } from '../terminal/useTerminalState';
import { connectBrowserTransport, type BrowserTransportConnection } from '../terminal/connect';
import type { TerminalGridSnapshot, TerminalGridStore } from '../terminal/gridStore';
import { encodeKeyEvent } from '../terminal/keymap';
import type { TerminalTransport } from '../transport/terminalTransport';
import { BackfillController } from '../terminal/backfillController';
import { cn } from '../lib/utils';

export type TerminalStatus = 'idle' | 'connecting' | 'connected' | 'error' | 'closed';

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
  const containerRef = useRef<HTMLDivElement | null>(null);
  const transportRef = useRef<TerminalTransport | null>(providedTransport ?? null);
  const connectionRef = useRef<BrowserTransportConnection | null>(null);
  const subscriptionRef = useRef<number | null>(null);
  const inputSeqRef = useRef(0);
  const [status, setStatus] = useState<TerminalStatus>(
    providedTransport ? 'connected' : 'idle',
  );
  const [error, setError] = useState<Error | null>(null);
  const [showIdlePlaceholder, setShowIdlePlaceholder] = useState(true);
  useEffect(() => {
    onStatusChange?.(status);
    if (status === 'connected') {
      setShowIdlePlaceholder(false);
    } else if (status === 'idle') {
      setShowIdlePlaceholder(true);
    }
  }, [status, onStatusChange]);
  const lines = useMemo(() => buildLines(snapshot, 600), [snapshot]);
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
      if (connectionRef.current) {
        connectionRef.current.close();
      }
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
    store.registerPrediction(seq, payload);
  };

  const wrapperClasses = cn(
    'relative flex h-full min-h-0 flex-col overflow-hidden',
    'rounded-[22px] border border-[#0f131a] bg-[#090d14]/95 shadow-[0_45px_120px_-70px_rgba(10,26,55,0.85)]',
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
    <div className={wrapperClasses}>
      <div className="pointer-events-none absolute inset-0">
        <div className="absolute inset-x-0 top-0 h-28 bg-gradient-to-b from-white/12 via-white/0 to-transparent opacity-20" aria-hidden />
        <div className="absolute inset-0 rounded-[22px] ring-1 ring-[#1f2736]/60" aria-hidden />
      </div>
      <header className="relative z-10 flex items-center justify-between gap-4 bg-[#111925]/95 px-6 py-3 text-[11px] font-medium uppercase tracking-[0.36em] text-[#9aa4bc]">
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
                  <path d="M4.1 4.1l3.8 3.8" stroke="#0f3d1d" strokeWidth="1.1" strokeLinecap="round" />
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
          minHeight: lineHeight * Math.max(1, minimumRows),
          height: '100%',
          maxHeight: '100%',
        }}
      >
        {showIdlePlaceholder ? (
          <IdlePlaceholder onConnectNotice={() => setShowIdlePlaceholder(false)} status={status} />
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
        break;
      case 'input_ack':
        store.clearPrediction(frame.seq);
        break;
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
  predicted?: boolean;
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
      if (snapshot.cursorRow === row.absolute && snapshot.cursorCol !== null) {
        const raw = Math.floor(Math.max(snapshot.cursorCol, 0));
        cursorCol = Number.isFinite(raw) ? raw : null;
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
    const className = cn('xterm-row', line.kind === 'pending' ? 'opacity-60' : undefined);
    return <div className={className}>{text}</div>;
  }

  const cursorCol = line.cursorCol ?? null;
  const baseStyleDef = styles.get(0) ?? { id: 0, fg: 0, bg: 0, attrs: 0 };

  return (
    <div className="xterm-row">
      {line.cells.map((cell, index) => {
        const styleDef = styles.get(cell.styleId);
        const isCursor = cursorCol !== null && cursorCol === index;
        let style = styleDef ? styleFromDefinition(styleDef, isCursor) : undefined;
        const predicted = cell.predicted === true;
        if (predicted) {
          const merged: CSSProperties = { ...(style ?? {}) };
          merged.textDecoration = appendTextDecoration(merged.textDecoration, 'underline');
          const existingOpacity = merged.opacity !== undefined ? Number(merged.opacity) : undefined;
          merged.opacity = existingOpacity !== undefined ? existingOpacity * 0.75 : 0.75;
          style = merged;
        }
        const char = cell.char === ' ' ? ' ' : cell.char;
        return (
          <span key={index} style={style} data-predicted={predicted || undefined}>
            {char}
          </span>
        );
      })}
      {cursorCol !== null && cursorCol >= line.cells.length ? (
        <span key="cursor" style={styleFromDefinition(baseStyleDef, true)}> </span>
      ) : null}
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

function IdlePlaceholder({ onConnectNotice, status }: { onConnectNotice: () => void; status: TerminalStatus }): JSX.Element {
  useEffect(() => {
    if (status === 'connected') {
      onConnectNotice();
    }
  }, [status, onConnectNotice]);
  return (
    <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center gap-2 bg-gradient-to-br from-[#0a101b]/92 via-[#0d1421]/92 to-[#05070b]/94 text-[13px] font-mono text-[#8f9ab5]">
      <div className="rounded-full border border-white/10 bg-[#141a28]/90 px-3 py-1 text-[11px] uppercase tracking-[0.4em] text-white/70">Terminal idle</div>
      <p className="text-xs text-white/40">Awaiting connection</p>
    </div>
  );
}
