import type { Update } from '../protocol/types';

const HIGH_SHIFT = 32;
const WORD = 2 ** HIGH_SHIFT;
const LOW_MASK = 0xffff_ffff;

export interface StyleDefinition {
  id: number;
  fg: number;
  bg: number;
  attrs: number;
}

export interface CellState {
  char: string;
  styleId: number;
  seq: number;
}

export interface LoadedRow {
  kind: 'loaded';
  absolute: number;
  latestSeq: number;
  cells: CellState[];
}

export interface PendingRow {
  kind: 'pending';
  absolute: number;
}

export interface MissingRow {
  kind: 'missing';
  absolute: number;
}

export type RowSlot = LoadedRow | PendingRow | MissingRow;

export interface TerminalGridSnapshot {
  baseRow: number;
  cols: number;
  rows: RowSlot[];
  styles: Map<number, StyleDefinition>;
  followTail: boolean;
  historyTrimmed: boolean;
  viewportTop: number;
  viewportHeight: number;
}

/**
 * Headless grid store that mirrors the semantics of the Rust client (GridRenderer).
 * The store is intentionally imperative and exposes a subscribe/notify API so React
 * integrations can wire into `useSyncExternalStore` or any other state layer.
 */
export class TerminalGridStore {
  private baseRow = 0;
  private cols = 0;
  private followTail = true;
  private historyTrimmed = false;
  private viewportTop = 0;
  private viewportHeight = 0;
  private readonly rows = new Map<number, RowSlot>();
  private readonly styles = new Map<number, StyleDefinition>();
  private readonly listeners = new Set<() => void>();
  private snapshotCache: TerminalGridSnapshot | null = null;
  private version = 0;

  constructor(initialCols = 0) {
    this.cols = initialCols;
    this.styles.set(0, { id: 0, fg: DEFAULT_COLOR, bg: DEFAULT_COLOR, attrs: 0 });
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  getSnapshot(): TerminalGridSnapshot {
    if (this.snapshotCache) {
      return this.snapshotCache;
    }
    const snapshot: TerminalGridSnapshot = {
      baseRow: this.baseRow,
      cols: this.cols,
      rows: Array.from(this.rows.values()).sort((a, b) => a.absolute - b.absolute),
      styles: new Map(this.styles),
      followTail: this.followTail,
      historyTrimmed: this.historyTrimmed,
      viewportTop: this.viewportTop,
      viewportHeight: this.viewportHeight,
    };
    this.snapshotCache = snapshot;
    return snapshot;
  }

  setGridSize(totalRows: number, cols: number): void {
    this.cols = Math.max(this.cols, cols);
    for (let offset = 0; offset < totalRows; offset += 1) {
      const absolute = this.baseRow + offset;
      if (!this.rows.has(absolute)) {
        this.rows.set(absolute, { kind: 'pending', absolute });
      }
    }
    this.invalidate();
    this.notify();
  }

  setViewport(top: number, height: number): void {
    if (top !== this.viewportTop || height !== this.viewportHeight) {
      this.viewportTop = Math.max(0, top);
      this.viewportHeight = Math.max(0, height);
      this.invalidate();
      this.notify();
    }
  }

  setFollowTail(enabled: boolean): void {
    if (this.followTail !== enabled) {
      this.followTail = enabled;
      this.invalidate();
      this.notify();
    }
  }

  setBaseRow(baseRow: number): void {
    if (baseRow === this.baseRow) {
      return;
    }
    const oldBase = this.baseRow;
    this.baseRow = baseRow;
    if (baseRow > oldBase) {
      for (const key of Array.from(this.rows.keys())) {
        if (key < baseRow) {
          this.rows.delete(key);
        }
      }
    }
    if (baseRow > 0) {
      this.historyTrimmed = true;
    }
    this.invalidate();
    this.notify();
  }

  setHistoryOrigin(baseRow: number): void {
    this.historyTrimmed = baseRow > 0;
    this.setBaseRow(baseRow);
  }

  applyUpdates(updates: Update[]): void {
    let mutated = false;
    for (const update of updates) {
      mutated = this.applyUpdate(update) || mutated;
    }
    if (mutated) {
      this.invalidate();
      this.notify();
    }
  }

  markRowPending(absolute: number): void {
    const existing = this.rows.get(absolute);
    if (existing?.kind === 'pending') {
      return;
    }
    this.rows.set(absolute, { kind: 'pending', absolute });
    this.invalidate();
  }

  markPendingRange(start: number, end: number): void {
    for (let row = start; row < end; row += 1) {
      this.markRowPending(row);
    }
    this.notify();
  }

  markRowMissing(absolute: number): void {
    const existing = this.rows.get(absolute);
    if (existing?.kind === 'missing') {
      return;
    }
    this.rows.set(absolute, { kind: 'missing', absolute });
    this.invalidate();
  }

  getRowText(absolute: number): string | undefined {
    const slot = this.rows.get(absolute);
    if (!slot || slot.kind !== 'loaded') {
      return undefined;
    }
    const chars = slot.cells.map((cell) => cell.char ?? ' ');
    while (chars.length && chars[chars.length - 1] === ' ') {
      chars.pop();
    }
    return chars.join('');
  }

  getRow(absolute: number): RowSlot | undefined {
    return this.rows.get(absolute);
  }

  private applyUpdate(update: Update): boolean {
    let mutated = false;
    switch (update.type) {
      case 'cell':
        mutated = this.applyCell(update.row, update.col, update.seq, update.cell);
        break;
      case 'rect':
        mutated = this.applyRect(update.rows, update.cols, update.seq, update.cell);
        break;
      case 'row':
        mutated = this.applyRow(update.row, update.seq, update.cells);
        break;
      case 'row_segment':
        mutated = this.applyRowSegment(update.row, update.startCol, update.seq, update.cells);
        break;
      case 'trim':
        mutated = this.applyTrim(update.start, update.count);
        break;
      case 'style':
        mutated = this.applyStyle(update.id, update.fg, update.bg, update.attrs);
        break;
      default:
        break;
    }
    return mutated;
  }

  private applyCell(row: number, col: number, seq: number, packed: number): boolean {
    const loaded = this.ensureLoadedRow(row);
    let mutated = this.extendRow(loaded, col + 1);
    const cell = loaded.cells[col]!;
    if (seq >= cell.seq) {
      const decoded = decodePackedCell(packed);
      cell.char = decoded.char;
      cell.styleId = decoded.styleId;
      cell.seq = seq;
      loaded.latestSeq = Math.max(loaded.latestSeq, seq);
      mutated = true;
    }
    this.touchRow(row);
    return mutated;
  }

  private applyRow(absoluteRow: number, seq: number, cells: number[]): boolean {
    const loaded = this.ensureLoadedRow(absoluteRow);
    const width = Math.max(cells.length, this.cols);
    let mutated = this.extendRow(loaded, width);
    for (let col = 0; col < width; col += 1) {
      const packed = cells[col];
      const cell = loaded.cells[col]!;
      if (packed === undefined) {
        if (seq >= cell.seq) {
          cell.char = ' ';
          cell.styleId = 0;
          cell.seq = seq;
          mutated = true;
        }
        continue;
      }
      if (seq >= cell.seq) {
        const decoded = decodePackedCell(packed);
        cell.char = decoded.char;
        cell.styleId = decoded.styleId;
        cell.seq = seq;
        mutated = true;
      }
    }
    loaded.latestSeq = Math.max(loaded.latestSeq, seq);
    this.touchRow(absoluteRow);
    return mutated;
  }

  private applyRowSegment(row: number, startCol: number, seq: number, cells: number[]): boolean {
    const loaded = this.ensureLoadedRow(row);
    const endCol = startCol + cells.length;
    let mutated = this.extendRow(loaded, endCol);
    for (let index = 0; index < cells.length; index += 1) {
      const col = startCol + index;
      const packed = cells[index]!;
      const cell = loaded.cells[col]!;
      if (seq >= cell.seq) {
        const decoded = decodePackedCell(packed);
        cell.char = decoded.char;
        cell.styleId = decoded.styleId;
        cell.seq = seq;
        mutated = true;
      }
    }
    loaded.latestSeq = Math.max(loaded.latestSeq, seq);
    this.touchRow(row);
    return mutated;
  }

  private applyRect(rowRange: [number, number], colRange: [number, number], seq: number, packed: number): boolean {
    let mutated = false;
    for (let row = rowRange[0]; row < rowRange[1]; row += 1) {
      const loaded = this.ensureLoadedRow(row);
      mutated = this.extendRow(loaded, colRange[1]) || mutated;
      for (let col = colRange[0]; col < colRange[1]; col += 1) {
        const cell = loaded.cells[col]!;
        if (seq >= cell.seq) {
          const decoded = decodePackedCell(packed);
          cell.char = decoded.char;
          cell.styleId = decoded.styleId;
          cell.seq = seq;
          mutated = true;
        }
      }
      loaded.latestSeq = Math.max(loaded.latestSeq, seq);
      this.touchRow(row);
    }
    return mutated;
  }

  private applyTrim(start: number, count: number): boolean {
    let mutated = false;
    const end = start + count;
    for (const key of Array.from(this.rows.keys())) {
      if (key >= start && key < end) {
        this.rows.delete(key);
        mutated = true;
      }
    }
    if (this.baseRow < end) {
      this.baseRow = end;
      mutated = true;
    }
    return mutated;
  }

  private applyStyle(id: number, fg: number, bg: number, attrs: number): boolean {
    this.styles.set(id, { id, fg, bg, attrs });
    return true;
  }

  private ensureLoadedRow(absolute: number): LoadedRow {
    const existing = this.rows.get(absolute);
    if (existing && existing.kind === 'loaded') {
      return existing;
    }
    const loaded: LoadedRow = {
      kind: 'loaded',
      absolute,
      latestSeq: 0,
      cells: createBlankRow(this.cols || DEFAULT_ROW_WIDTH),
    };
    this.rows.set(absolute, loaded);
    return loaded;
  }

  private extendRow(row: LoadedRow, requiredCols: number): boolean {
    if (row.cells.length >= requiredCols) {
      return false;
    }
    for (let index = row.cells.length; index < requiredCols; index += 1) {
      row.cells.push(createBlankCell());
    }
    this.cols = Math.max(this.cols, requiredCols);
    return true;
  }

  private touchRow(absolute: number): void {
    if (!this.followTail || this.viewportHeight === 0) {
      return;
    }
    const bottomRow = this.viewportTop + this.viewportHeight - 1;
    if (absolute > bottomRow) {
      this.viewportTop = Math.max(0, absolute - this.viewportHeight + 1);
      this.invalidate();
    }
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
    this.version += 1;
  }
}

const DEFAULT_COLOR = 0x000000;
const DEFAULT_ROW_WIDTH = 80;

function createBlankCell(): CellState {
  return { char: ' ', styleId: 0, seq: 0 };
}

function createBlankRow(width: number): CellState[] {
  return Array.from({ length: Math.max(1, width) }, () => createBlankCell());
}

function decodePackedCell(packed: number): { char: string; styleId: number } {
  const codePoint = Math.floor(packed / WORD);
  const styleBits = packed - codePoint * WORD;
  const char = safeFromCodePoint(codePoint);
  return { char, styleId: styleBits & LOW_MASK };
}

function safeFromCodePoint(codePoint: number): string {
  try {
    return String.fromCodePoint(codePoint);
  } catch {
    return '\uFFFD';
  }
}
