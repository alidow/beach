import type { CursorFrame, Update } from '../protocol/types';

declare global {
  interface Window {
    __BEACH_TRACE?: boolean;
  }
}

function trace(...parts: unknown[]): void {
  if (typeof window !== 'undefined' && window.__BEACH_TRACE) {
    console.debug('[beach-trace][cache]', ...parts);
  }
}

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

export interface PredictedCell {
  char: string;
  seq: number;
}

interface PredictedPosition {
  row: number;
  col: number;
}

export interface PredictedCursorState {
  row: number;
  col: number;
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
  cursorRow: number | null;
  cursorCol: number | null;
  cursorVisible: boolean;
  cursorBlink: boolean;
  cursorSeq: number | null;
  cursorAuthoritative: boolean;
  predictedCursor: PredictedCursorState | null;
  visibleRows(limit?: number): RowSlot[];
  getPrediction(row: number, col: number): PredictedCell | null;
  predictionsForRow(row: number): Array<{ col: number; cell: PredictedCell }>;
}

interface TerminalGridCacheOptions {
  initialCols?: number;
  maxHistory?: number;
}

interface DebugUpdateContext {
  origin: string | null;
  update: Update;
  authoritative: boolean;
}

type CursorHint =
  | { kind: 'exact'; row: number; col: number }
  | { kind: 'row_width'; row: number };

export interface ApplyUpdatesOptions {
  authoritative?: boolean;
  origin?: string;
  cursor?: CursorFrame | null;
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
  private cursorRow: number | null = null;
  private cursorCol: number | null = null;
  private cursorSeq: number | null = null;
  private cursorVisible = true;
  private cursorBlink = true;
  private cursorFeatureEnabled = false;
  private cursorAuthoritative = false;
  private cursorAuthoritativePending = false;
  private predictedCursor: PredictedCursorState | null = null;
  private predictions = new Map<number, Map<number, PredictedCell>>();
  private pendingPredictions = new Map<number, PredictedPosition[]>();
  private debugContext: DebugUpdateContext | null = null;

  constructor(options: TerminalGridCacheOptions = {}) {
    this.maxHistory = options.maxHistory ?? DEFAULT_HISTORY_LIMIT;
    this.cols = Math.max(0, options.initialCols ?? 0);
    this.styles.set(0, { id: 0, fg: DEFAULT_COLOR, bg: DEFAULT_COLOR, attrs: 0 });
    trace('init', { maxHistory: this.maxHistory, cols: this.cols });
    this.cursorSeq = null;
    this.cursorVisible = true;
    this.cursorBlink = true;
    this.cursorFeatureEnabled = false;
    this.cursorAuthoritative = false;
    this.cursorAuthoritativePending = false;
    this.predictedCursor = null;
  }

  reset(): void {
    trace('reset');
    this.baseRow = 0;
    this.cols = 0;
    this.rows = [];
    this.followTail = true;
    this.viewportTop = 0;
    this.viewportHeight = 0;
    this.historyTrimmed = false;
    this.knownBaseRow = null;
    this.styles = new Map([[0, { id: 0, fg: DEFAULT_COLOR, bg: DEFAULT_COLOR, attrs: 0 }]]);
    this.cursorRow = null;
    this.cursorCol = null;
    this.cursorSeq = null;
    this.cursorVisible = true;
    this.cursorBlink = true;
    this.cursorFeatureEnabled = false;
    this.cursorAuthoritative = false;
    this.cursorAuthoritativePending = false;
    this.predictedCursor = null;
    this.predictions.clear();
    this.pendingPredictions.clear();
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
    if (mutated) {
      this.clampCursor();
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
      if (this.cursorRow !== null && this.cursorRow < this.baseRow) {
        this.cursorRow = this.baseRow;
        this.cursorCol = 0;
      }
      if (this.predictedCursor && this.predictedCursor.row < this.baseRow) {
        this.predictedCursor = null;
        mutated = true;
      }
      mutated = this.prunePredictionsBelow(this.baseRow) || mutated;
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
    this.clampCursor();
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

  applyUpdates(updates: Update[], options: ApplyUpdatesOptions = {}): boolean {
    const { authoritative = false, origin, cursor } = options;

    if (cursor && this.cursorFeatureEnabled) {
      this.cursorAuthoritativePending = true;
    }

    let mutated = false;
    let baseAdjusted = false;
    let cursorChanged = false;
    let cursorHintSeen = false;
    const originLabel = origin ?? null;
    trace('applyUpdates start', {
      count: updates.length,
      authoritative,
      origin: originLabel,
      cursor,
    });

    if (updates.length > 0) {
      for (const update of updates) {
        this.debugContext = {
          origin: originLabel,
          update,
          authoritative,
        };
        const beforeWidth = this.debugRowWidthForUpdate(update);
        trace('applyUpdates update', {
          type: update.type,
          row: extractUpdateRow(update),
          authoritative,
        });
        baseAdjusted = this.observeBounds(update, authoritative) || baseAdjusted;
        mutated = this.applyGridUpdate(update) || mutated;
        const hint = this.cursorAuthoritative || this.cursorAuthoritativePending ? null : this.cursorHint(update);
        if (hint) {
          cursorHintSeen = true;
          cursorChanged = this.applyCursorHint(hint) || cursorChanged;
        }
        this.logCursorDebug(update, hint, beforeWidth);
        this.debugContext = null;
      }
    }

    if (cursor && this.cursorFeatureEnabled) {
      cursorChanged = this.applyCursorFrame(cursor) || cursorChanged;
    }

    const changed = mutated || baseAdjusted || cursorChanged || cursorHintSeen;
    trace('applyUpdates complete', { changed, mutated, baseAdjusted, cursorChanged, cursorHintSeen });
    return changed;
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
    this.clearPredictionsForRow(absolute);
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
    this.clearPredictionsForRow(absolute);
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
      const tailHeadroom = Math.max(0, height - 1);
      const oldestTracked = highestLoaded - tailHeadroom;
      startAbsolute = Math.max(this.baseRow, oldestTracked);
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
      cursorRow: this.cursorRow,
      cursorCol: this.cursorCol,
      cursorVisible: this.cursorVisible,
      cursorBlink: this.cursorBlink,
      cursorSeq: this.cursorSeq,
      cursorAuthoritative: this.cursorAuthoritative,
      predictedCursor: this.predictedCursor,
      visibleRows: (limit?: number) => this.visibleRows(limit),
      getPrediction: (row: number, col: number) => this.getPrediction(row, col),
      predictionsForRow: (row: number) => this.predictionsForRow(row),
    };
  }

  private applyGridUpdate(update: Update): boolean {
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

  private cursorHint(update: Update): CursorHint | null {
    switch (update.type) {
      case 'cell':
        return { kind: 'exact', row: update.row, col: update.col + 1 };
      case 'row': {
        const col = this.inferRowCursorColumn(update.cells);
        return { kind: 'exact', row: update.row, col };
      }
      case 'rect': {
        const [rowStart, rowEnd] = update.rows;
        if (rowEnd <= rowStart) {
          return null;
        }
        const targetRow = rowEnd - 1;
        if (targetRow < 0) {
          return null;
        }
        return { kind: 'row_width', row: targetRow };
      }
      case 'row_segment': {
        if (update.cells.length === 0) {
          return { kind: 'row_width', row: update.row };
        }
        const col = update.startCol + update.cells.length;
        return { kind: 'exact', row: update.row, col };
      }
      default:
        return null;
    }
  }

  private applyCursorHint(hint: CursorHint): boolean {
    const previousRow = this.cursorRow;
    const previousCol = this.cursorCol;
    switch (hint.kind) {
      case 'exact': {
        const row = Math.max(0, Math.floor(hint.row));
        const col = Math.max(0, Math.floor(hint.col));
        this.cursorRow = row;
        this.cursorCol = col;
        break;
      }
      case 'row_width': {
        const row = Math.max(0, Math.floor(hint.row));
        this.cursorRow = row;
        this.cursorCol = this.rowDisplayWidth(row);
        break;
      }
      default:
        break;
    }
    this.clampCursor();
    return this.cursorRow !== previousRow || this.cursorCol !== previousCol;
  }

  enableCursorSupport(enabled: boolean): boolean {
    if (this.cursorFeatureEnabled === enabled) {
      return false;
    }
    this.cursorFeatureEnabled = enabled;
    if (!enabled) {
      this.cursorAuthoritative = false;
      this.cursorAuthoritativePending = false;
      this.cursorSeq = null;
      this.cursorVisible = true;
      this.cursorBlink = true;
      this.predictedCursor = null;
    }
    return true;
  }

  private applyCursorFrame(frame: CursorFrame): boolean {
    const row = Math.max(0, Math.floor(frame.row));
    const col = Math.max(0, Math.floor(frame.col));
    const prevRow = this.cursorRow;
    const prevCol = this.cursorCol;
    const prevSeq = this.cursorSeq;
    const prevVisible = this.cursorVisible;
    const prevBlink = this.cursorBlink;
    const prevPredicted = this.predictedCursor;

    this.ensureRowRange(row, row + 1);
    this.cursorRow = row;
    this.cursorCol = col;
    this.cursorSeq = frame.seq;
    this.cursorVisible = frame.visible;
    this.cursorBlink = frame.blink;
    this.cursorAuthoritative = true;
    this.cursorAuthoritativePending = false;

    if (this.cursorRow !== null && this.cursorRow < this.baseRow) {
      this.cursorRow = this.baseRow;
    }
    this.clampCursor();

    const viewportMoved = this.touchRow(this.cursorRow ?? row);

    if (this.predictedCursor && this.predictedCursor.seq <= frame.seq) {
      this.predictedCursor = null;
    }

    return (
      viewportMoved ||
      prevRow !== this.cursorRow ||
      prevCol !== this.cursorCol ||
      prevSeq !== this.cursorSeq ||
      prevVisible !== this.cursorVisible ||
      prevBlink !== this.cursorBlink ||
      prevPredicted !== this.predictedCursor
    );
  }

  private rowDisplayWidth(absolute: number): number {
    if (absolute < this.baseRow) {
      return 0;
    }
    const index = absolute - this.baseRow;
    if (index < 0 || index >= this.rows.length) {
      return 0;
    }
    const slot = this.rows[index];
    if (!slot || slot.kind !== 'loaded') {
      return 0;
    }
    for (let col = slot.cells.length - 1; col >= 0; col -= 1) {
      const cell = slot.cells[col]!;
      if (cell.char !== ' ' || cell.styleId !== 0) {
        return col + 1;
      }
    }
    return 0;
  }

  private clampCursor(): void {
    if (this.cursorRow === null || this.cursorCol === null) {
      return;
    }
    if (this.cursorRow < this.baseRow) {
      this.cursorRow = this.baseRow;
    }
    if (this.cols <= 0) {
      this.cursorCol = 0;
      return;
    }
    const maxCol = Math.max(this.cols, 0);
    this.cursorCol = clamp(this.cursorCol, 0, maxCol);
  }

  private inferRowCursorColumn(cells: number[]): number {
    if (!cells || cells.length === 0) {
      return 0;
    }
    let lastDefined = -1;
    for (let index = cells.length - 1; index >= 0; index -= 1) {
      if (cells[index] !== undefined) {
        lastDefined = index;
        break;
      }
    }
    return Math.max(0, lastDefined + 1);
  }

  private debugRowWidthForUpdate(update: Update): number | null {
    const row = extractUpdateRow(update);
    if (row === null) {
      return null;
    }
    return this.rowDisplayWidth(row);
  }

  private logCursorDebug(update: Update, hint: CursorHint | null, beforeWidth: number | null): void {
    if (!isCursorDebuggingEnabled()) {
      return;
    }
    const summary = summarizeUpdate(update);
    const row = hint ? hint.row : extractUpdateRow(update);
    const afterWidth = row === null ? null : this.rowDisplayWidth(row);
    console.log('[grid.cursor]', {
      update: summary,
      hint,
      beforeWidth,
      afterWidth,
      cursorRow: this.cursorRow,
      cursorCol: this.cursorCol,
    });
  }

  private applyCell(row: number, col: number, seq: number, packed: number): boolean {
    const loaded = this.ensureLoadedRow(row);
    let mutated = this.extendRow(loaded, col + 1);
    if (this.ensureCols(col + 1)) {
      mutated = true;
    }
    const cell = loaded.cells[col]!;
    mutated = this.clearPredictionAt(row, col) || mutated;
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
    const debugChars: string[] = [];
    for (let col = 0; col < width; col += 1) {
      const packed = cells[col];
      const cell = loaded.cells[col]!;
      mutated = this.clearPredictionAt(row, col) || mutated;
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
      if (typeof window !== 'undefined' && window.__BEACH_TRACE && col < 16) {
        debugChars[col] = cell.char ?? ' ';
      }
    }
    if (typeof window !== 'undefined' && window.__BEACH_TRACE && row < this.baseRow + 5) {
      trace('applyRow result', {
        row,
        seq,
        width,
        preview: debugChars.join(''),
      });
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
      mutated = this.clearPredictionAt(row, col) || mutated;
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
        mutated = this.clearPredictionAt(row, col) || mutated;
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
        mutated = this.clearPredictionAt(row, col) || mutated;
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
    if (this.cursorRow !== null && this.cursorRow >= start && this.cursorRow < end) {
      this.cursorRow = end;
      this.cursorCol = 0;
      mutated = true;
    }
    if (this.predictedCursor && this.predictedCursor.row >= start && this.predictedCursor.row < end) {
      this.predictedCursor = null;
      mutated = true;
    }
    this.reindexRows();
    this.clampCursor();
    mutated = this.prunePredictionsBelow(this.baseRow) || mutated;
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

  registerPrediction(seq: number, data: Uint8Array): boolean {
    if (!Number.isFinite(seq) || seq <= 0) {
      return false;
    }
    if (!data || data.length === 0 || data.length > 32) {
      return false;
    }

    let mutated = false;
    let cursorMoved = false;

    let workingRow: number | null;
    let workingCol: number | null;

    if (this.cursorFeatureEnabled && this.cursorAuthoritative) {
      workingRow = this.predictedCursor?.row ?? this.cursorRow;
      workingCol = this.predictedCursor?.col ?? this.cursorCol;
    } else {
      workingRow = this.cursorRow;
      workingCol = this.cursorCol;
    }

    if (workingRow === null || workingCol === null) {
      const fallbackRow = this.findHighestLoadedRow();
      workingRow = fallbackRow ?? this.baseRow;
      workingCol = this.rowDisplayWidth(workingRow);
    }

    if (workingRow === null || !Number.isFinite(workingRow)) {
      workingRow = this.baseRow;
    }
    if (workingCol === null || !Number.isFinite(workingCol)) {
      workingCol = 0;
    }

    let currentRow = workingRow;
    let currentCol = workingCol;

    const positions: PredictedPosition[] = [];

    for (const byte of data) {
      if (byte === 0x0d) {
        if (currentCol !== 0) {
          cursorMoved = true;
        }
        currentCol = 0;
        continue;
      }
      if (byte === 0x0a) {
        currentRow += 1;
        currentCol = 0;
        cursorMoved = true;
        continue;
      }
      if (byte <= 0x1f || byte === 0x7f) {
        continue;
      }

      const row = currentRow;
      const col = currentCol;
      const char = String.fromCharCode(byte);
      mutated = this.setPrediction(row, col, seq, char) || mutated;
      positions.push({ row, col });
      const next = this.nextCursorPosition(row, col, char);
      currentRow = next.row;
      currentCol = next.col;
      cursorMoved = true;
    }

    this.pendingPredictions.delete(seq);
    if (positions.length > 0) {
      this.pendingPredictions.set(seq, positions);
      if (this.pendingPredictions.size > 256) {
        mutated = this.clearAllPredictions() || mutated;
      }
    }

    if (this.cursorFeatureEnabled && this.cursorAuthoritative) {
      currentRow = Math.max(this.baseRow, currentRow);
      currentCol = Math.max(0, currentCol);
      const newPredicted: PredictedCursorState = { row: currentRow, col: currentCol, seq };
      const prev = this.predictedCursor;
      const changed =
        !prev || prev.row !== newPredicted.row || prev.col !== newPredicted.col || prev.seq !== newPredicted.seq;
      this.predictedCursor = newPredicted;
      mutated = mutated || changed || cursorMoved;
    } else {
      currentRow = Math.max(this.baseRow, currentRow);
      currentCol = Math.max(0, currentCol);
      this.cursorRow = currentRow;
      this.cursorCol = currentCol;
      this.clampCursor();
      mutated = mutated || cursorMoved;
    }

    return mutated || positions.length > 0 || cursorMoved;
  }

  clearPredictionSeq(seq: number): boolean {
    const positions = this.pendingPredictions.get(seq);
    let cursorCleared = false;
    if (this.predictedCursor && this.predictedCursor.seq === seq) {
      this.predictedCursor = null;
      cursorCleared = true;
    }
    if (!positions || positions.length === 0) {
      this.pendingPredictions.delete(seq);
      return cursorCleared;
    }
    this.pendingPredictions.delete(seq);
    let mutated = false;
    for (const { row, col } of positions) {
      mutated = this.clearPredictionAt(row, col) || mutated;
    }
    return mutated || cursorCleared;
  }

  clearAllPredictions(): boolean {
    if (
      this.predictions.size === 0 &&
      this.pendingPredictions.size === 0 &&
      this.predictedCursor === null
    ) {
      return false;
    }
    this.predictions.clear();
    this.pendingPredictions.clear();
    this.predictedCursor = null;
    return true;
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

  private setPrediction(row: number, col: number, seq: number, char: string): boolean {
    const loaded = this.ensureLoadedRow(row);
    this.extendRow(loaded, col + 1);
    let rowPredictions = this.predictions.get(row);
    if (!rowPredictions) {
      rowPredictions = new Map();
      this.predictions.set(row, rowPredictions);
    }
    const existing = rowPredictions.get(col);
    if (existing && existing.char === char && existing.seq === seq) {
      return false;
    }
    rowPredictions.set(col, { char, seq });
    return true;
  }

  private clearPredictionAt(row: number, col: number): boolean {
    const rowPredictions = this.predictions.get(row);
    if (!rowPredictions) {
      return false;
    }
    const existing = rowPredictions.get(col);
    if (!existing) {
      return false;
    }
    rowPredictions.delete(col);
    if (rowPredictions.size === 0) {
      this.predictions.delete(row);
    }
    const positions = this.pendingPredictions.get(existing.seq);
    if (positions) {
      const filtered = positions.filter((pos) => !(pos.row === row && pos.col === col));
      if (filtered.length === 0) {
        this.pendingPredictions.delete(existing.seq);
      } else if (filtered.length !== positions.length) {
        this.pendingPredictions.set(existing.seq, filtered);
      }
    }
    return true;
  }

  private clearPredictionsForRow(row: number): boolean {
    const rowPredictions = this.predictions.get(row);
    if (!rowPredictions || rowPredictions.size === 0) {
      return false;
    }
    const cols = Array.from(rowPredictions.keys());
    let mutated = false;
    for (const col of cols) {
      mutated = this.clearPredictionAt(row, col) || mutated;
    }
    return mutated;
  }

  private prunePredictionsBelow(row: number): boolean {
    let mutated = false;
    for (const predRow of Array.from(this.predictions.keys())) {
      if (predRow < row) {
        mutated = this.clearPredictionsForRow(predRow) || mutated;
      }
    }
    if (this.predictedCursor && this.predictedCursor.row < row) {
      this.predictedCursor = null;
      mutated = true;
    }
    return mutated;
  }

  private getPrediction(row: number, col: number): PredictedCell | null {
    const rowPredictions = this.predictions.get(row);
    if (!rowPredictions) {
      return null;
    }
    return rowPredictions.get(col) ?? null;
  }

  private predictionsForRow(row: number): Array<{ col: number; cell: PredictedCell }> {
    const rowPredictions = this.predictions.get(row);
    if (!rowPredictions || rowPredictions.size === 0) {
      return [];
    }
    return Array.from(rowPredictions.entries())
      .sort((a, b) => a[0] - b[0])
      .map(([col, cell]) => ({ col, cell }));
  }

  private nextCursorPosition(row: number, col: number, char: string): { row: number; col: number } {
    if (char === '\n') {
      return { row: row + 1, col: 0 };
    }
    if (char === '\r') {
      return { row, col: 0 };
    }
    return { row, col: col + 1 };
  }

  private advanceCursorForChar(char: string): void {
    const currentRow = this.cursorRow ?? this.baseRow;
    const currentCol = this.cursorCol ?? 0;
    const next = this.nextCursorPosition(currentRow, currentCol, char);
    this.cursorRow = next.row;
    this.cursorCol = next.col;
    this.clampCursor();
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
    if (mutated) {
      this.prunePredictionsBelow(this.baseRow);
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
        const width = this.rowDisplayWidth(slot.absolute);
        if (width > 0) {
          return slot.absolute;
        }
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

function isCursorDebuggingEnabled(): boolean {
  try {
    const global = globalThis as { __BEACH_CURSOR_DEBUG__?: unknown };
    return Boolean(global.__BEACH_CURSOR_DEBUG__);
  } catch {
    return false;
  }
}

function summarizeUpdate(update: Update): Record<string, unknown> {
  switch (update.type) {
    case 'cell': {
      const decoded = decodePackedCell(update.cell);
      return {
        type: update.type,
        row: update.row,
        col: update.col,
        seq: update.seq,
        char: decoded.char,
      };
    }
    case 'rect':
      return {
        type: update.type,
        rows: update.rows,
        cols: update.cols,
        seq: update.seq,
      };
    case 'row':
      return {
        type: update.type,
        row: update.row,
        seq: update.seq,
        cells: update.cells.length,
      };
    case 'row_segment':
      return {
        type: update.type,
        row: update.row,
        startCol: update.startCol,
        seq: update.seq,
        cells: update.cells.length,
      };
    case 'trim':
      return {
        type: update.type,
        start: update.start,
        count: update.count,
        seq: update.seq,
      };
    case 'style':
      return {
        type: update.type,
        id: update.id,
        seq: update.seq,
      };
    default:
      return { type: (update as { type: string }).type };
  }
}
