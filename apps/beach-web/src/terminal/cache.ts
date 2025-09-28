import type { Update } from '../protocol/types';

const HIGH_SHIFT = 32;
const WORD = 2 ** HIGH_SHIFT;
const LOW_MASK = 0xffff_ffff;
const DEFAULT_COLOR = 0x000000;
const DEFAULT_ROW_WIDTH = 80;
const DEFAULT_HISTORY_LIMIT = 5_000;

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
  visibleRows(limit?: number): RowSlot[];
}

interface TerminalGridCacheOptions {
  initialCols?: number;
  maxHistory?: number;
}

export class TerminalGridCache {
  private readonly maxHistory: number;
  private baseRow = 0;
  private cols = 0;
  private rows: RowSlot[] = [];
  private followTail = true;
  private viewportTop = 0;
  private viewportHeight = 0;
  private historyTrimmed = false;
  private knownBaseRow: number | null = null;
  private styles = new Map<number, StyleDefinition>();

  constructor(options: TerminalGridCacheOptions = {}) {
    this.maxHistory = options.maxHistory ?? DEFAULT_HISTORY_LIMIT;
    this.cols = Math.max(0, options.initialCols ?? 0);
    this.styles.set(0, { id: 0, fg: DEFAULT_COLOR, bg: DEFAULT_COLOR, attrs: 0 });
  }

  reset(): void {
    this.baseRow = 0;
    this.cols = 0;
    this.rows = [];
    this.followTail = true;
    this.viewportTop = 0;
    this.viewportHeight = 0;
    this.historyTrimmed = false;
    this.knownBaseRow = null;
    this.styles = new Map([[0, { id: 0, fg: DEFAULT_COLOR, bg: DEFAULT_COLOR, attrs: 0 }]]);
  }

  setGridSize(totalRows: number, cols: number): boolean {
    let mutated = false;
    if (this.ensureCols(cols)) {
      mutated = true;
    }
    const start = this.baseRow;
    const end = start + totalRows;
    if (this.ensureRowRange(start, end)) {
      mutated = true;
    }
    return mutated;
  }

  setViewport(top: number, height: number): boolean {
    const clampedTop = Math.max(0, top);
    const clampedHeight = Math.max(0, height);
    if (clampedTop === this.viewportTop && clampedHeight === this.viewportHeight) {
      return false;
    }
    this.viewportTop = clampedTop;
    this.viewportHeight = clampedHeight;
    return true;
  }

  setFollowTail(enabled: boolean): boolean {
    if (this.followTail === enabled) {
      return false;
    }
    this.followTail = enabled;
    return true;
  }

  setBaseRow(baseRow: number): boolean {
    if (baseRow === this.baseRow) {
      return false;
    }
    if (this.rows.length === 0) {
      this.baseRow = baseRow;
      return true;
    }
    let mutated = false;
    if (baseRow > this.baseRow) {
      const delta = baseRow - this.baseRow;
      if (delta >= this.rows.length) {
        this.rows = [];
      } else {
        this.rows.splice(0, delta);
      }
      this.baseRow = baseRow;
      this.historyTrimmed = this.historyTrimmed || baseRow > 0;
      mutated = true;
    } else {
      const newRows: RowSlot[] = [];
      for (let absolute = baseRow; absolute < this.baseRow; absolute += 1) {
        newRows.push(createPendingRow(absolute));
      }
      this.rows = newRows.concat(this.rows);
      this.baseRow = baseRow;
      mutated = true;
    }
    this.trimToCapacity();
    this.reindexRows();
    return mutated;
  }

  setHistoryOrigin(baseRow: number): boolean {
    const changed = this.setBaseRow(baseRow);
    if (!this.historyTrimmed && baseRow > 0) {
      this.historyTrimmed = true;
      return true;
    }
    return changed;
  }

  applyUpdates(updates: Update[], authoritative = false): boolean {
    let mutated = false;
    let baseAdjusted = false;
    for (const update of updates) {
      baseAdjusted = this.observeBounds(update, authoritative) || baseAdjusted;
      mutated = this.applyUpdate(update) || mutated;
    }
    return mutated || baseAdjusted;
  }

  markRowPending(absolute: number): boolean {
    if (!Number.isFinite(absolute) || absolute < 0) {
      return false;
    }
    this.ensureRowRange(absolute, absolute + 1);
    const index = absolute - this.baseRow;
    const existing = this.rows[index];
    if (existing && existing.kind === 'pending') {
      return false;
    }
    this.rows[index] = createPendingRow(absolute);
    return true;
  }

  markRowMissing(absolute: number): boolean {
    if (!Number.isFinite(absolute) || absolute < 0) {
      return false;
    }
    this.ensureRowRange(absolute, absolute + 1);
    const index = absolute - this.baseRow;
    const existing = this.rows[index];
    if (existing && existing.kind === 'missing') {
      return false;
    }
    this.rows[index] = createMissingRow(absolute);
    return true;
  }

  markPendingRange(start: number, end: number): boolean {
    if (end <= start) {
      return false;
    }
    let mutated = false;
    this.ensureRowRange(start, end);
    for (let absolute = start; absolute < end; absolute += 1) {
      const index = absolute - this.baseRow;
      const existing = this.rows[index];
      if (!existing || existing.kind !== 'pending') {
        this.rows[index] = createPendingRow(absolute);
        mutated = true;
      }
    }
    return mutated;
  }

  firstGapBetween(start: number, end: number): number | null {
    if (end <= start) {
      return null;
    }
    for (let absolute = start; absolute < end; absolute += 1) {
      const slot = this.getRow(absolute);
      if (!slot || slot.kind !== 'loaded') {
        return absolute;
      }
    }
    return null;
  }

  getRow(absolute: number): RowSlot | undefined {
    if (absolute < this.baseRow) {
      return undefined;
    }
    const index = absolute - this.baseRow;
    if (index < 0 || index >= this.rows.length) {
      return undefined;
    }
    return cloneRowSlot(this.rows[index]!);
  }

  getRowText(absolute: number): string | undefined {
    const slot = this.getRow(absolute);
    if (!slot || slot.kind !== 'loaded') {
      return undefined;
    }
    return trimRowText(slot.cells);
  }

  visibleRows(limit = this.viewportHeight || this.rows.length): RowSlot[] {
    if (this.rows.length === 0) {
      return [];
    }
    const fallbackHeight = Math.min(limit, Math.max(1, this.rows.length));
    const requestedHeight = this.viewportHeight > 0 ? this.viewportHeight : fallbackHeight;
    const height = Math.max(1, Math.min(limit, requestedHeight));

    const highestLoaded = this.findHighestLoadedRow();
    let startAbsolute: number;
    if (this.followTail && highestLoaded !== null) {
      startAbsolute = Math.max(this.baseRow, highestLoaded - height + 1);
    } else if (this.followTail) {
      startAbsolute = this.baseRow;
    } else {
      startAbsolute = clamp(
        this.viewportTop,
        this.baseRow,
        Math.max(this.baseRow, this.baseRow + this.rows.length - height),
      );
    }

    const rows: RowSlot[] = [];
    for (let offset = 0; offset < height; offset += 1) {
      const absolute = startAbsolute + offset;
      const slot = this.getRow(absolute) ?? createMissingRow(absolute);
      rows.push(slot);
      if (rows.length >= limit) {
        break;
      }
    }
    return rows;
  }

  snapshot(): TerminalGridSnapshot {
    const rows = this.rows.map((slot) => cloneRowSlot(slot));
    const styles = new Map(this.styles);
    return {
      baseRow: this.baseRow,
      cols: this.cols,
      rows,
      styles,
      followTail: this.followTail,
      historyTrimmed: this.historyTrimmed,
      viewportTop: this.viewportTop,
      viewportHeight: this.viewportHeight,
      visibleRows: (limit?: number) => this.visibleRows(limit),
    };
  }

  private applyUpdate(update: Update): boolean {
    switch (update.type) {
      case 'cell':
        return this.applyCell(update.row, update.col, update.seq, update.cell);
      case 'row':
        return this.applyRow(update.row, update.seq, update.cells);
      case 'row_segment':
        return this.applyRowSegment(update.row, update.startCol, update.seq, update.cells);
      case 'rect':
        return this.applyRect(update.rows, update.cols, update.seq, update.cell);
      case 'trim':
        return this.applyTrim(update.start, update.count);
      case 'style':
        return this.applyStyle(update.id, update.fg, update.bg, update.attrs);
      default:
        return false;
    }
  }

  private applyCell(row: number, col: number, seq: number, packed: number): boolean {
    const loaded = this.ensureLoadedRow(row);
    let mutated = this.extendRow(loaded, col + 1);
    if (this.ensureCols(col + 1)) {
      mutated = true;
    }
    const cell = loaded.cells[col]!;
    if (seq >= cell.seq) {
      const decoded = decodePackedCell(packed);
      cell.char = decoded.char;
      cell.styleId = decoded.styleId;
      cell.seq = seq;
      loaded.latestSeq = Math.max(loaded.latestSeq, seq);
      this.touchRow(row);
      return true;
    }
    const viewportMoved = this.touchRow(row);
    return mutated || viewportMoved;
  }

  private applyRow(row: number, seq: number, cells: number[]): boolean {
    const loaded = this.ensureLoadedRow(row);
    const width = Math.max(cells.length, this.cols);
    let mutated = this.extendRow(loaded, width);
    if (this.ensureCols(width)) {
      mutated = true;
    }
    for (let col = 0; col < width; col += 1) {
      const packed = cells[col];
      const cell = loaded.cells[col]!;
      if (packed === undefined) {
        if (seq >= cell.seq && (cell.char !== ' ' || cell.styleId !== 0)) {
          cell.char = ' ';
          cell.styleId = 0;
          cell.seq = seq;
          mutated = true;
        }
        continue;
      }
      if (seq >= cell.seq) {
        const decoded = decodePackedCell(packed);
        if (cell.char !== decoded.char || cell.styleId !== decoded.styleId || cell.seq !== seq) {
          cell.char = decoded.char;
          cell.styleId = decoded.styleId;
          cell.seq = seq;
          mutated = true;
        }
      }
    }
    loaded.latestSeq = Math.max(loaded.latestSeq, seq);
    const viewportMoved = this.touchRow(row);
    return mutated || viewportMoved;
  }

  private applyRowSegment(row: number, startCol: number, seq: number, cells: number[]): boolean {
    const loaded = this.ensureLoadedRow(row);
    const endCol = startCol + cells.length;
    let mutated = this.extendRow(loaded, endCol);
    if (this.ensureCols(endCol)) {
      mutated = true;
    }
    for (let index = 0; index < cells.length; index += 1) {
      const col = startCol + index;
      const packed = cells[index]!;
      const cell = loaded.cells[col]!;
      if (seq >= cell.seq) {
        const decoded = decodePackedCell(packed);
        if (cell.char !== decoded.char || cell.styleId !== decoded.styleId || cell.seq !== seq) {
          cell.char = decoded.char;
          cell.styleId = decoded.styleId;
          cell.seq = seq;
          mutated = true;
        }
      }
    }
    if (startCol === 0) {
      for (let col = endCol; col < loaded.cells.length; col += 1) {
        const cell = loaded.cells[col]!;
        if (seq >= cell.seq && (cell.char !== ' ' || cell.styleId !== 0)) {
          cell.char = ' ';
          cell.styleId = 0;
          cell.seq = seq;
          mutated = true;
        }
      }
    }
    loaded.latestSeq = Math.max(loaded.latestSeq, seq);
    const viewportMoved = this.touchRow(row);
    return mutated || viewportMoved;
  }

  private applyRect(rowRange: [number, number], colRange: [number, number], seq: number, packed: number): boolean {
    let mutated = false;
    const decoded = decodePackedCell(packed);
    const width = colRange[1];
    if (this.ensureCols(width)) {
      mutated = true;
    }
    for (let row = rowRange[0]; row < rowRange[1]; row += 1) {
      const loaded = this.ensureLoadedRow(row);
      const extended = this.extendRow(loaded, width);
      mutated = extended || mutated;
      for (let col = colRange[0]; col < colRange[1]; col += 1) {
        const cell = loaded.cells[col]!;
        if (seq >= cell.seq) {
          if (cell.char !== decoded.char || cell.styleId !== decoded.styleId || cell.seq !== seq) {
            cell.char = decoded.char;
            cell.styleId = decoded.styleId;
            cell.seq = seq;
            mutated = true;
          }
        }
      }
      loaded.latestSeq = Math.max(loaded.latestSeq, seq);
      const viewportMoved = this.touchRow(row);
      mutated = mutated || viewportMoved;
    }
    return mutated;
  }

  private applyTrim(start: number, count: number): boolean {
    if (count <= 0) {
      return false;
    }
    const end = start + count;
    let mutated = false;
    if (end <= this.baseRow) {
      return false;
    }
    const removalStart = Math.max(start, this.baseRow);
    const removalEnd = Math.min(end, this.baseRow + this.rows.length);
    if (removalStart < removalEnd) {
      const startIndex = removalStart - this.baseRow;
      const removeCount = removalEnd - removalStart;
      this.rows.splice(startIndex, removeCount);
      mutated = removeCount > 0;
    }
    if (end > this.baseRow) {
      this.baseRow = Math.max(this.baseRow, end);
      mutated = true;
    }
    this.historyTrimmed = this.historyTrimmed || end > 0;
    if (this.knownBaseRow === null || this.knownBaseRow < end) {
      this.knownBaseRow = end;
    }
    this.reindexRows();
    return mutated;
  }

  private applyStyle(id: number, fg: number, bg: number, attrs: number): boolean {
    const existing = this.styles.get(id);
    if (existing && existing.fg === fg && existing.bg === bg && existing.attrs === attrs) {
      return false;
    }
    this.styles.set(id, { id, fg, bg, attrs });
    return true;
  }

  private ensureLoadedRow(absolute: number): LoadedRow {
    this.ensureRowRange(absolute, absolute + 1);
    const index = absolute - this.baseRow;
    const existing = this.rows[index];
    if (existing && existing.kind === 'loaded') {
      this.extendRow(existing, this.cols || DEFAULT_ROW_WIDTH);
      return existing;
    }
    const initialWidth = this.cols > 0 ? this.cols : DEFAULT_ROW_WIDTH;
    const loaded: LoadedRow = {
      kind: 'loaded',
      absolute,
      latestSeq: 0,
      cells: createBlankRow(initialWidth),
    };
    this.rows[index] = loaded;
    return loaded;
  }

  private ensureCols(requiredCols: number): boolean {
    if (requiredCols <= this.cols) {
      return false;
    }
    this.cols = requiredCols;
    let mutated = false;
    for (const slot of this.rows) {
      if (slot && slot.kind === 'loaded') {
        mutated = this.extendRow(slot, requiredCols) || mutated;
      }
    }
    return mutated;
  }

  private extendRow(row: LoadedRow, requiredCols: number): boolean {
    if (row.cells.length >= requiredCols) {
      return false;
    }
    for (let index = row.cells.length; index < requiredCols; index += 1) {
      row.cells.push(createBlankCell());
    }
    return true;
  }

  private touchRow(absolute: number): boolean {
    if (!this.followTail || this.viewportHeight === 0) {
      return false;
    }
    const bottomRow = this.viewportTop + this.viewportHeight - 1;
    if (absolute > bottomRow) {
      const nextTop = Math.max(0, absolute - this.viewportHeight + 1);
      if (nextTop !== this.viewportTop) {
        this.viewportTop = nextTop;
        return true;
      }
    }
    return false;
  }

  private observeBounds(update: Update, authoritative: boolean): boolean {
    const minRow = extractUpdateRow(update);
    if (minRow === null) {
      return false;
    }
    if (authoritative) {
      const base = this.knownBaseRow === null ? minRow : Math.min(this.knownBaseRow, minRow);
      if (this.knownBaseRow !== base) {
        this.knownBaseRow = base;
      }
      return this.setBaseRow(base);
    }
    if (minRow < this.baseRow) {
      return this.setBaseRow(minRow);
    }
    return false;
  }

  private ensureRowRange(start: number, end: number): boolean {
    if (end <= start) {
      return false;
    }
    if (this.rows.length === 0) {
      this.baseRow = start;
    }
    let mutated = false;
    if (start < this.baseRow) {
      const newRows: RowSlot[] = [];
      for (let absolute = start; absolute < this.baseRow; absolute += 1) {
        newRows.push(createPendingRow(absolute));
      }
      this.rows = newRows.concat(this.rows);
      this.baseRow = start;
      mutated = true;
    }
    if (end > this.baseRow + this.rows.length) {
      for (let absolute = this.baseRow + this.rows.length; absolute < end; absolute += 1) {
        this.rows.push(createPendingRow(absolute));
      }
      mutated = true;
    }
    if (this.trimToCapacity()) {
      mutated = true;
    }
    this.reindexRows();
    return mutated;
  }

  private trimToCapacity(): boolean {
    let mutated = false;
    while (this.rows.length > this.maxHistory) {
      this.rows.shift();
      this.baseRow += 1;
      this.historyTrimmed = true;
      mutated = true;
    }
    return mutated;
  }

  private reindexRows(): void {
    for (let index = 0; index < this.rows.length; index += 1) {
      const absolute = this.baseRow + index;
      const slot = this.rows[index];
      if (!slot) {
        continue;
      }
      slot.absolute = absolute;
    }
  }

  private findHighestLoadedRow(): number | null {
    for (let index = this.rows.length - 1; index >= 0; index -= 1) {
      const slot = this.rows[index];
      if (slot && slot.kind === 'loaded') {
        return slot.absolute;
      }
    }
    return null;
  }
}

function createBlankCell(): CellState {
  return { char: ' ', styleId: 0, seq: 0 };
}

function createBlankRow(width: number): CellState[] {
  return Array.from({ length: Math.max(1, width) }, () => createBlankCell());
}

function createPendingRow(absolute: number): PendingRow {
  return { kind: 'pending', absolute };
}

function createMissingRow(absolute: number): MissingRow {
  return { kind: 'missing', absolute };
}

function cloneRowSlot(slot: RowSlot): RowSlot {
  switch (slot.kind) {
    case 'loaded':
      return {
        kind: 'loaded',
        absolute: slot.absolute,
        latestSeq: slot.latestSeq,
        cells: slot.cells.map((cell) => ({ ...cell })),
      };
    case 'pending':
      return { kind: 'pending', absolute: slot.absolute };
    case 'missing':
    default:
      return { kind: 'missing', absolute: slot.absolute };
  }
}

function trimRowText(cells: CellState[]): string {
  const chars = cells.map((cell) => cell.char ?? ' ');
  while (chars.length && chars[chars.length - 1] === ' ') {
    chars.pop();
  }
  return chars.join('');
}

function decodePackedCell(packed: number): { char: string; styleId: number } {
  let codePoint = Math.floor(packed / WORD);
  let styleBits = packed - codePoint * WORD;

  if (codePoint === 0 && packed > 0 && packed < WORD) {
    codePoint = packed;
    styleBits = 0;
  }

  const char = safeFromCodePoint(codePoint);
  return { char, styleId: styleBits & LOW_MASK };
}

function extractUpdateRow(update: Update): number | null {
  switch (update.type) {
    case 'cell':
    case 'row':
    case 'row_segment':
      return update.row;
    case 'rect':
      return update.rows[0] ?? null;
    case 'trim':
    case 'style':
    default:
      return null;
  }
}

function clamp(value: number, min: number, max: number): number {
  if (value < min) {
    return min;
  }
  if (value > max) {
    return max;
  }
  return value;
}

function safeFromCodePoint(codePoint: number): string {
  try {
    return String.fromCodePoint(codePoint);
  } catch {
    return '\uFFFD';
  }
}
