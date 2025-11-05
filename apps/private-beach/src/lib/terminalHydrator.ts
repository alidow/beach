'use client';

import { TerminalGridStore } from '../../../beach-surfer/src/terminal/gridStore';
import type { Update, CursorFrame } from '../../../beach-surfer/src/protocol/types';
import type { TerminalViewerState } from '../hooks/terminalViewerTypes';

const WORD = 2 ** 32;
const DEFAULT_STYLE_ID = 0;

export type CellStylePayload = {
  id: number;
  fg?: number | null;
  bg?: number | null;
  attrs?: number | null;
};

export type StyledCell = {
  ch: string | null | undefined;
  style?: CellStylePayload | null;
};

export type TerminalFramePayload = {
  type: 'terminal_full';
  lines?: string[] | null;
  styled_lines?: StyledCell[][] | null;
  styles?: CellStylePayload[] | null;
  rows?: number | null;
  cols?: number | null;
  cursor?: { row?: number | null; col?: number | null } | null;
  base_row?: number | null;
};

export type TerminalStateDiff = {
  sequence: number;
  payload: TerminalFramePayload;
};

type HydrationOptions = {
  viewportRows?: number;
};

function sanitizeStyleId(raw: unknown, fallback = DEFAULT_STYLE_ID): number {
  if (typeof raw === 'number' && Number.isFinite(raw)) {
    const normalized = Math.trunc(raw);
    return normalized >= 0 ? normalized : fallback;
  }
  return fallback;
}

function extractChar(raw: unknown): string {
  if (typeof raw === 'string' && raw.length > 0) {
    const [first] = Array.from(raw);
    if (first && first.length > 0) {
      return first;
    }
  }
  return ' ';
}

function packCell(char: string, styleId: number): number {
  const codePoint = char.codePointAt(0) ?? 32;
  const style = sanitizeStyleId(styleId);
  return codePoint * WORD + (style >>> 0);
}

function inferCols(payload: TerminalFramePayload): number {
  if (typeof payload.cols === 'number' && Number.isFinite(payload.cols) && payload.cols > 0) {
    return Math.trunc(payload.cols);
  }
  if (Array.isArray(payload.styled_lines) && payload.styled_lines.length > 0) {
    const maxStyled = payload.styled_lines.reduce((max, row) => {
      if (!Array.isArray(row)) return max;
      return Math.max(max, row.length);
    }, 0);
    if (maxStyled > 0) {
      return maxStyled;
    }
  }
  if (Array.isArray(payload.lines) && payload.lines.length > 0) {
    const maxPlain = payload.lines.reduce((max, line) => {
      if (typeof line !== 'string') return max;
      return Math.max(max, Array.from(line).length);
    }, 0);
    if (maxPlain > 0) {
      return maxPlain;
    }
  }
  return 0;
}

function inferRows(payload: TerminalFramePayload): number {
  if (typeof payload.rows === 'number' && Number.isFinite(payload.rows) && payload.rows > 0) {
    return Math.trunc(payload.rows);
  }
  if (Array.isArray(payload.styled_lines)) {
    return payload.styled_lines.length;
  }
  if (Array.isArray(payload.lines)) {
    return payload.lines.length;
  }
  return 0;
}

function buildStyleUpdates(
  styles: CellStylePayload[] | null | undefined,
  usedStyleIds: Set<number>,
  sequence: number,
): Update[] {
  const updates: Update[] = [];
  const seen = new Set<number>();
  if (Array.isArray(styles)) {
    for (const entry of styles) {
      if (!entry || typeof entry !== 'object') {
        continue;
      }
      const styleId = sanitizeStyleId(entry.id, DEFAULT_STYLE_ID);
      if (seen.has(styleId)) {
        continue;
      }
      seen.add(styleId);
      usedStyleIds.add(styleId);
      updates.push({
        type: 'style',
        id: styleId,
        seq: sequence,
        fg: typeof entry.fg === 'number' ? entry.fg : 0,
        bg: typeof entry.bg === 'number' ? entry.bg : 0,
        attrs: typeof entry.attrs === 'number' ? entry.attrs : 0,
      });
    }
  }
  if (!seen.has(DEFAULT_STYLE_ID)) {
    usedStyleIds.add(DEFAULT_STYLE_ID);
    updates.push({
      type: 'style',
      id: DEFAULT_STYLE_ID,
      seq: sequence,
      fg: 0,
      bg: 0,
      attrs: 0,
    });
  }
  return updates;
}

function buildRowFromStyledCells(
  row: StyledCell[],
  cols: number,
  usedStyleIds: Set<number>,
): number[] {
  const cells: number[] = [];
  for (let col = 0; col < cols; col += 1) {
    const cell = row[col];
    if (!cell) {
      cells.push(packCell(' ', DEFAULT_STYLE_ID));
      continue;
    }
    const styleId = sanitizeStyleId(cell.style?.id, DEFAULT_STYLE_ID);
    usedStyleIds.add(styleId);
    const char = extractChar(cell.ch);
    cells.push(packCell(char, styleId));
  }
  return cells;
}

function buildRowFromPlainText(line: string, cols: number, usedStyleIds: Set<number>): number[] {
  const runes = Array.from(line);
  const cells: number[] = [];
  usedStyleIds.add(DEFAULT_STYLE_ID);
  for (let col = 0; col < cols; col += 1) {
    const char = col < runes.length ? runes[col] ?? ' ' : ' ';
    cells.push(packCell(char, DEFAULT_STYLE_ID));
  }
  return cells;
}

function buildCursorFrame(cursor: TerminalFramePayload['cursor'], sequence: number): CursorFrame | null {
  if (!cursor || typeof cursor !== 'object') {
    return null;
  }
  const row = typeof cursor.row === 'number' ? cursor.row : null;
  const col = typeof cursor.col === 'number' ? cursor.col : null;
  if (row == null || col == null) {
    return null;
  }
  return {
    row,
    col,
    seq: sequence,
    visible: true,
    blink: true,
  };
}

export function hydrateTerminalStoreFromDiff(
  store: TerminalGridStore,
  diff: TerminalStateDiff,
  options: HydrationOptions = {},
): boolean {
  if (!diff || typeof diff !== 'object' || !diff.payload) {
    return false;
  }
  const payload = diff.payload;
  const rows = inferRows(payload);
  const cols = inferCols(payload);
  if (!(rows > 0 && cols > 0)) {
    return false;
  }

  const usedStyleIds = new Set<number>();
  const updates: Update[] = [];
  updates.push(...buildStyleUpdates(payload.styles ?? null, usedStyleIds, diff.sequence));

  const styledLines = Array.isArray(payload.styled_lines) ? payload.styled_lines : null;
  const plainLines = Array.isArray(payload.lines) ? payload.lines : null;
  const baseRowRaw = payload.base_row;
  const baseRow =
    typeof baseRowRaw === 'number' && Number.isFinite(baseRowRaw) && baseRowRaw >= 0
      ? Math.min(Math.floor(baseRowRaw), Number.MAX_SAFE_INTEGER)
      : 0;

  for (let rowIndex = 0; rowIndex < rows; rowIndex += 1) {
    let cells: number[] | null = null;
    if (styledLines && styledLines[rowIndex] && Array.isArray(styledLines[rowIndex])) {
      cells = buildRowFromStyledCells(styledLines[rowIndex] as StyledCell[], cols, usedStyleIds);
    } else if (plainLines && typeof plainLines[rowIndex] === 'string') {
      cells = buildRowFromPlainText(plainLines[rowIndex] as string, cols, usedStyleIds);
    } else {
      cells = buildRowFromPlainText('', cols, usedStyleIds);
    }
    updates.push({
      type: 'row',
      row: baseRow + rowIndex,
      seq: diff.sequence,
      cells,
    });
  }

  store.reset();
  store.setHistoryOrigin(baseRow);
  store.setGridSize(rows, cols);
  store.setFollowTail(false);
  store.applyUpdates(updates, { authoritative: true, origin: 'state-diff-hydrate' });

  const viewportRows = Math.max(
    1,
    Math.min(rows, typeof options.viewportRows === 'number' && options.viewportRows > 0 ? options.viewportRows : rows),
  );
  const totalAbsoluteRows = baseRow + rows;
  const viewportTop = Math.max(baseRow, totalAbsoluteRows - viewportRows);
  store.setViewport(viewportTop, viewportRows);

  const cursorFrame = buildCursorFrame(payload.cursor, diff.sequence);
  if (cursorFrame) {
    store.applyCursorFrame(cursorFrame);
  }

  return true;
}

export function buildViewerStateFromTerminalDiff(
  diff: TerminalStateDiff,
  options: HydrationOptions = {},
): TerminalViewerState | null {
  const colsHint =
    (typeof diff.payload?.cols === 'number' && Number.isFinite(diff.payload.cols) && diff.payload.cols > 0
      ? Math.trunc(diff.payload.cols)
      : undefined) ?? undefined;
  const store = new TerminalGridStore(colsHint ?? 80);
  const hydrated = hydrateTerminalStoreFromDiff(store, diff, options);
  if (!hydrated) {
    return null;
  }
  return {
    store,
    transport: null,
    transportVersion: 0,
    connecting: false,
    error: null,
    status: 'connected',
    secureSummary: null,
    latencyMs: null,
  };
}

export function makeDiffFromLines(lines: readonly string[], sequence = 0): TerminalStateDiff {
  const sanitized = Array.isArray(lines) ? Array.from(lines, (line) => (typeof line === 'string' ? line : '')) : [];
  if (sanitized.length === 0) {
    sanitized.push('');
  }
  const cols = sanitized.reduce((max, line) => Math.max(max, Array.from(line).length), 0);
  return {
    sequence,
    payload: {
      type: 'terminal_full',
      lines: sanitized,
      rows: sanitized.length,
      cols,
      styled_lines: null,
      styles: null,
      cursor: null,
      base_row: 0,
    },
  };
}

function asNumber(value: unknown): number | undefined {
  if (typeof value === 'number' && Number.isFinite(value)) {
    return value;
  }
  if (typeof value === 'string' && value.trim().length > 0) {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) {
      return parsed;
    }
  }
  return undefined;
}

type QueueItem = {
  value: unknown;
  sequenceHint?: number;
};

export function extractTerminalStateDiff(input: unknown): TerminalStateDiff | null {
  const visited = new Set<unknown>();
  const queue: QueueItem[] = [{ value: input }];

  while (queue.length > 0) {
    const { value, sequenceHint } = queue.shift()!;
    if (value == null) {
      continue;
    }
    if (typeof value !== 'object') {
      continue;
    }
    if (visited.has(value)) {
      continue;
    }
    visited.add(value);

    if (Array.isArray(value)) {
      for (const entry of value) {
        queue.push({ value: entry, sequenceHint });
      }
      continue;
    }

    const record = value as Record<string, unknown>;
    const ownSequence =
      asNumber(record.sequence) ??
      asNumber((record as { seq?: unknown }).seq) ??
      asNumber((record as { sequence_number?: unknown }).sequence_number);
    const nextSequenceHint = ownSequence ?? sequenceHint;

    const payloadCandidate = record.payload;
    if (payloadCandidate && typeof payloadCandidate === 'object' && !Array.isArray(payloadCandidate)) {
      const payloadRecord = payloadCandidate as Record<string, unknown>;
      if (payloadRecord.type === 'terminal_full') {
        return {
          sequence: ownSequence ?? sequenceHint ?? 0,
          payload: payloadRecord as TerminalFramePayload,
        };
      }
    }

    if (record.type === 'terminal_full') {
      return {
        sequence: ownSequence ?? sequenceHint ?? 0,
        payload: record as unknown as TerminalFramePayload,
      };
    }

    for (const child of Object.values(record)) {
      queue.push({ value: child, sequenceHint: nextSequenceHint });
    }
  }

  return null;
}
