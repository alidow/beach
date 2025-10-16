import type { CursorFrame, Update } from '../protocol/types';
import { TerminalGridCache } from './cache';
import type { ApplyUpdatesOptions, RowSlot, TerminalGridSnapshot } from './cache';

declare global {
  interface Window {
    __BEACH_TRACE?: boolean;
  }
}

function trace(...parts: unknown[]): void {
  if (typeof window !== 'undefined' && window.__BEACH_TRACE) {
    console.debug('[beach-trace][gridStore]', ...parts);
  }
}

export type {
  CellState,
  LoadedRow,
  MissingRow,
  PendingRow,
  PredictedCell,
  PredictedCursorState,
  RowSlot,
  StyleDefinition,
  TerminalGridSnapshot,
} from './cache';

/**
 * Headless grid store that mirrors the semantics of the Rust client (GridRenderer).
 * The store uses an imperative cache internally and exposes a subscribe/notify API so React
 * integrations can hook into `useSyncExternalStore` or any other state layer.
 */
export class TerminalGridStore {
  private readonly cache: TerminalGridCache;
  private readonly listeners = new Set<() => void>();
  private snapshotCache: TerminalGridSnapshot | null = null;

  constructor(initialCols = 0, maxHistory?: number) {
    this.cache = new TerminalGridCache({ initialCols, maxHistory });
  }

  reset(): void {
    this.cache.reset();
    this.invalidate();
    this.notify();
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  getSnapshot(): TerminalGridSnapshot {
    if (this.snapshotCache) {
      return this.snapshotCache;
    }
    const snapshot = this.cache.snapshot();
    this.snapshotCache = snapshot;
    return snapshot;
  }

  setGridSize(totalRows: number, cols: number): void {
    if (this.cache.setGridSize(totalRows, cols)) {
      this.invalidate();
      this.notify();
    }
  }

  setViewport(top: number, height: number): void {
    if (this.cache.setViewport(top, height)) {
      this.invalidate();
      this.notify();
    }
  }

  setFollowTail(enabled: boolean): void {
    if (this.cache.setFollowTail(enabled)) {
      this.invalidate();
      this.notify();
    }
  }

  setBaseRow(baseRow: number): void {
    if (this.cache.setBaseRow(baseRow)) {
      this.invalidate();
      this.notify();
    }
  }

  setHistoryOrigin(baseRow: number): void {
    if (this.cache.setHistoryOrigin(baseRow)) {
      this.invalidate();
      this.notify();
    }
  }

  applyUpdates(updates: Update[], options: ApplyUpdatesOptions = {}): void {
    const hasCursor = Boolean(options.cursor);
    if (updates.length === 0 && !hasCursor) {
      trace('applyUpdates skipped (no updates, no cursor)', options);
      return;
    }
    trace('applyUpdates start', {
      count: updates.length,
      authoritative: options.authoritative,
      origin: options.origin,
      cursor: options.cursor ? { row: options.cursor.row, col: options.cursor.col, seq: options.cursor.seq } : null,
    });
    if (this.cache.applyUpdates(updates, options)) {
      trace('applyUpdates mutated');
      this.invalidate();
      this.notify();
    } else {
      trace('applyUpdates no-op');
    }
  }

  setCursorSupport(enabled: boolean): void {
    trace('cursorSupport set', enabled);
    if (this.cache.enableCursorSupport(enabled)) {
      this.invalidate();
      this.notify();
    }
  }

  applyCursorFrame(cursor: CursorFrame): void {
    trace('applyCursorFrame', cursor);
    this.applyUpdates([], { cursor });
  }

  markRowPending(absolute: number): void {
    if (this.cache.markRowPending(absolute)) {
      this.invalidate();
      this.notify();
    }
  }

  markPendingRange(start: number, end: number): void {
    if (this.cache.markPendingRange(start, end)) {
      this.invalidate();
      this.notify();
    }
  }

  markRowMissing(absolute: number): void {
    if (this.cache.markRowMissing(absolute)) {
      this.invalidate();
      this.notify();
    }
  }

  registerPrediction(seq: number, data: Uint8Array): boolean {
    const changed = this.cache.registerPrediction(seq, data);
    if (changed) {
      this.invalidate();
      this.notify();
    }
    return changed;
  }

  clearPrediction(seq: number): void {
    if (this.cache.clearPredictionSeq(seq)) {
      this.invalidate();
      this.notify();
    }
  }

  ackPrediction(seq: number, timestampMs: number): void {
    if (this.cache.ackPrediction(seq, timestampMs)) {
      this.invalidate();
      this.notify();
    }
  }

  pruneAckedPredictions(nowMs: number, graceMs: number): void {
    if (this.cache.pruneAckedPredictions(nowMs, graceMs)) {
      this.invalidate();
      this.notify();
    }
  }

  clearAllPredictions(): void {
    if (this.cache.clearAllPredictions()) {
      this.invalidate();
      this.notify();
    }
  }

  firstGapBetween(start: number, end: number): number | null {
    return this.cache.firstGapBetween(start, end);
  }

  getRowText(absolute: number): string | undefined {
    return this.cache.getRowText(absolute);
  }

  getRow(absolute: number): RowSlot | undefined {
    return this.cache.getRow(absolute);
  }

  visibleRows(limit?: number): RowSlot[] {
    return this.cache.visibleRows(limit);
  }

  private notify(): void {
    if (this.listeners.size === 0) {
      return;
    }
    for (const listener of this.listeners) {
      listener();
    }
  }

  private invalidate(): void {
    this.snapshotCache = null;
  }
}
