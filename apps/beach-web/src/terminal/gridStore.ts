import type { Update } from '../protocol/types';
import {
  TerminalGridCache,
  type CellState,
  type LoadedRow,
  type MissingRow,
  type PendingRow,
  type RowSlot,
  type StyleDefinition,
  type TerminalGridSnapshot,
} from './cache';

export type {
  CellState,
  LoadedRow,
  MissingRow,
  PendingRow,
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

  applyUpdates(updates: Update[], authoritative = false): void {
    if (updates.length === 0) {
      return;
    }
    if (this.cache.applyUpdates(updates, authoritative)) {
      this.invalidate();
      this.notify();
    }
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
