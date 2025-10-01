import {
  type ClientFrame,
  type CursorFrame,
  type HostFrame,
  type LaneBudgetFrame,
  type SyncConfigFrame,
  type Update,
  Lane,
  PROTOCOL_VERSION,
} from './types';
import { MAX_SAFE_U53, readBytes, readVarUint, writeBytes, writeVarUint } from './varint';

const VERSION_BITS = 3;
const VERSION_MASK = 0b1110_0000;
const TYPE_MASK = 0b0001_1111;

const HOST_KIND_HEARTBEAT = 0;
const HOST_KIND_HELLO = 1;
const HOST_KIND_GRID = 2;
const HOST_KIND_SNAPSHOT = 3;
const HOST_KIND_SNAPSHOT_COMPLETE = 4;
const HOST_KIND_DELTA = 5;
const HOST_KIND_INPUT_ACK = 6;
const HOST_KIND_SHUTDOWN = 7;
const HOST_KIND_HISTORY_BACKFILL = 8;
const HOST_KIND_CURSOR = 9;

const CLIENT_KIND_INPUT = 0;
const CLIENT_KIND_RESIZE = 1;
const CLIENT_KIND_REQUEST_BACKFILL = 2;

const UPDATE_KIND_CELL = 0;
const UPDATE_KIND_RECT = 1;
const UPDATE_KIND_ROW = 2;
const UPDATE_KIND_SEGMENT = 3;
const UPDATE_KIND_TRIM = 4;
const UPDATE_KIND_STYLE = 5;

interface Cursor {
  value: number;
}

function createCursor(): Cursor {
  return { value: 0 };
}

function ensureSafe(value: number, label: string): number {
  if (!Number.isFinite(value) || value < 0 || value > MAX_SAFE_U53) {
    throw new RangeError(`${label} out of range: ${value}`);
  }
  return value;
}

function ensureByte(value: number, label: string): number {
  if (!Number.isInteger(value) || value < 0 || value > 0xff) {
    throw new RangeError(`${label} must be a byte, got: ${value}`);
  }
  return value;
}

function writeHeader(kind: number, out: number[]): void {
  const version = PROTOCOL_VERSION & ((1 << VERSION_BITS) - 1);
  out.push(((version << 5) | (kind & TYPE_MASK)) & 0xff);
}

function readHeader(bytes: Uint8Array, cursor: Cursor): number {
  const byte = readU8(bytes, cursor);
  const version = (byte & VERSION_MASK) >> 5;
  if (version !== (PROTOCOL_VERSION & ((1 << VERSION_BITS) - 1))) {
    throw new Error(`invalid protocol version: ${version}`);
  }
  return byte & TYPE_MASK;
}

function writeU8(value: number, out: number[]): void {
  if (!Number.isInteger(value) || value < 0 || value > 0xff) {
    throw new RangeError(`invalid u8: ${value}`);
  }
  out.push(value);
}

function readU8(bytes: Uint8Array, cursor: Cursor): number {
  if (cursor.value >= bytes.length) {
    throw new RangeError('unexpected end of input while reading byte');
  }
  return bytes[cursor.value++];
}

function writeBool(value: boolean, out: number[]): void {
  writeU8(value ? 1 : 0, out);
}

function readBool(bytes: Uint8Array, cursor: Cursor): boolean {
  const byte = readU8(bytes, cursor);
  if (byte === 0) return false;
  if (byte === 1) return true;
  throw new RangeError(`invalid boolean byte: ${byte}`);
}

function writeLane(lane: Lane, out: number[]): void {
  switch (lane) {
    case Lane.Foreground:
    case Lane.Recent:
    case Lane.History:
      writeU8(lane, out);
      return;
    default:
      throw new RangeError(`invalid lane value: ${lane}`);
  }
}

function readLane(bytes: Uint8Array, cursor: Cursor): Lane {
  const lane = readU8(bytes, cursor);
  switch (lane) {
    case Lane.Foreground:
    case Lane.Recent:
    case Lane.History:
      return lane;
    default:
      throw new RangeError(`invalid lane byte: ${lane}`);
  }
}

function writeSyncConfig(config: SyncConfigFrame, out: number[]): void {
  writeVarUint(config.snapshotBudgets.length, out);
  for (const budget of config.snapshotBudgets) {
    writeLane(budget.lane, out);
    writeVarUint(ensureSafe(budget.maxUpdates, 'maxUpdates'), out);
  }
  writeVarUint(ensureSafe(config.deltaBudget, 'deltaBudget'), out);
  writeVarUint(ensureSafe(config.heartbeatMs, 'heartbeatMs'), out);
  writeVarUint(ensureSafe(config.initialSnapshotLines, 'initialSnapshotLines'), out);
}

function readSyncConfig(bytes: Uint8Array, cursor: Cursor): SyncConfigFrame {
  const count = readVarUint(bytes, cursor);
  const snapshotBudgets: LaneBudgetFrame[] = [];
  for (let index = 0; index < count; index += 1) {
    const lane = readLane(bytes, cursor);
    const maxUpdates = readVarUint(bytes, cursor);
    snapshotBudgets.push({ lane, maxUpdates });
  }
  const deltaBudget = readVarUint(bytes, cursor);
  const heartbeatMs = readVarUint(bytes, cursor);
  const initialSnapshotLines = readVarUint(bytes, cursor);
  return { snapshotBudgets, deltaBudget, heartbeatMs, initialSnapshotLines };
}

function writeUpdates(updates: Update[], out: number[]): void {
  writeVarUint(updates.length, out);
  for (const update of updates) {
    switch (update.type) {
      case 'cell': {
        writeU8(UPDATE_KIND_CELL, out);
        writeVarUint(ensureSafe(update.row, 'row'), out);
        writeVarUint(ensureSafe(update.col, 'col'), out);
        writeVarUint(ensureSafe(update.seq, 'seq'), out);
        writeVarUint(ensureSafe(update.cell, 'cell'), out);
        break;
      }
      case 'rect': {
        writeU8(UPDATE_KIND_RECT, out);
        writeVarUint(ensureSafe(update.rows[0], 'rows[0]'), out);
        writeVarUint(ensureSafe(update.rows[1], 'rows[1]'), out);
        writeVarUint(ensureSafe(update.cols[0], 'cols[0]'), out);
        writeVarUint(ensureSafe(update.cols[1], 'cols[1]'), out);
        writeVarUint(ensureSafe(update.seq, 'seq'), out);
        writeVarUint(ensureSafe(update.cell, 'cell'), out);
        break;
      }
      case 'row': {
        writeU8(UPDATE_KIND_ROW, out);
        writeVarUint(ensureSafe(update.row, 'row'), out);
        writeVarUint(ensureSafe(update.seq, 'seq'), out);
        writeVarUint(ensureSafe(update.cells.length, 'row cell count'), out);
        for (const cell of update.cells) {
          writeVarUint(ensureSafe(cell, 'cell'), out);
        }
        break;
      }
      case 'row_segment': {
        writeU8(UPDATE_KIND_SEGMENT, out);
        writeVarUint(ensureSafe(update.row, 'row'), out);
        writeVarUint(ensureSafe(update.startCol, 'startCol'), out);
        writeVarUint(ensureSafe(update.seq, 'seq'), out);
        writeVarUint(ensureSafe(update.cells.length, 'segment cell count'), out);
        for (const cell of update.cells) {
          writeVarUint(ensureSafe(cell, 'cell'), out);
        }
        break;
      }
      case 'trim': {
        writeU8(UPDATE_KIND_TRIM, out);
        writeVarUint(ensureSafe(update.start, 'start'), out);
        writeVarUint(ensureSafe(update.count, 'count'), out);
        writeVarUint(ensureSafe(update.seq, 'seq'), out);
        break;
      }
      case 'style': {
        writeU8(UPDATE_KIND_STYLE, out);
        writeVarUint(ensureSafe(update.id, 'style id'), out);
        writeVarUint(ensureSafe(update.seq, 'seq'), out);
        writeVarUint(ensureSafe(update.fg, 'fg'), out);
        writeVarUint(ensureSafe(update.bg, 'bg'), out);
        writeU8(ensureByte(update.attrs, 'attrs'), out);
        break;
      }
      default: {
        const neverUpdate: never = update;
        throw new Error(`unsupported update type ${(neverUpdate as Update).type}`);
      }
    }
  }
}

function writeCursorFrame(cursor: CursorFrame, out: number[]): void {
  writeVarUint(ensureSafe(cursor.row, 'cursor row'), out);
  writeVarUint(ensureSafe(cursor.col, 'cursor col'), out);
  writeVarUint(ensureSafe(cursor.seq, 'cursor seq'), out);
  writeBool(cursor.visible, out);
  writeBool(cursor.blink, out);
}

function readUpdates(bytes: Uint8Array, cursor: Cursor): Update[] {
  const count = readVarUint(bytes, cursor);
  const updates: Update[] = [];
  for (let index = 0; index < count; index += 1) {
    const tag = readU8(bytes, cursor);
    switch (tag) {
      case UPDATE_KIND_CELL: {
        const row = readVarUint(bytes, cursor);
        const col = readVarUint(bytes, cursor);
        const seq = readVarUint(bytes, cursor);
        const cell = readVarUint(bytes, cursor);
        updates.push({ type: 'cell', row, col, seq, cell });
        break;
      }
      case UPDATE_KIND_RECT: {
        const rowStart = readVarUint(bytes, cursor);
        const rowEnd = readVarUint(bytes, cursor);
        const colStart = readVarUint(bytes, cursor);
        const colEnd = readVarUint(bytes, cursor);
        const seq = readVarUint(bytes, cursor);
        const cell = readVarUint(bytes, cursor);
        updates.push({
          type: 'rect',
          rows: [rowStart, rowEnd],
          cols: [colStart, colEnd],
          seq,
          cell,
        });
        break;
      }
      case UPDATE_KIND_ROW: {
        const row = readVarUint(bytes, cursor);
        const seq = readVarUint(bytes, cursor);
        const len = readVarUint(bytes, cursor);
        const cells: number[] = [];
        for (let i = 0; i < len; i += 1) {
          cells.push(readVarUint(bytes, cursor));
        }
        updates.push({ type: 'row', row, seq, cells });
        break;
      }
      case UPDATE_KIND_SEGMENT: {
        const row = readVarUint(bytes, cursor);
        const startCol = readVarUint(bytes, cursor);
        const seq = readVarUint(bytes, cursor);
        const len = readVarUint(bytes, cursor);
        const cells: number[] = [];
        for (let i = 0; i < len; i += 1) {
          cells.push(readVarUint(bytes, cursor));
        }
        updates.push({ type: 'row_segment', row, startCol, seq, cells });
        break;
      }
      case UPDATE_KIND_TRIM: {
        const start = readVarUint(bytes, cursor);
        const count = readVarUint(bytes, cursor);
        const seq = readVarUint(bytes, cursor);
        updates.push({ type: 'trim', start, count, seq });
        break;
      }
      case UPDATE_KIND_STYLE: {
        const id = readVarUint(bytes, cursor);
        const seq = readVarUint(bytes, cursor);
        const fg = readVarUint(bytes, cursor);
        const bg = readVarUint(bytes, cursor);
        const attrs = readU8(bytes, cursor);
        updates.push({ type: 'style', id, seq, fg, bg, attrs });
        break;
      }
      default:
        throw new RangeError(`unknown update tag: ${tag}`);
    }
  }
  return updates;
}

function readCursorFrame(bytes: Uint8Array, cursor: Cursor): CursorFrame {
  const row = readVarUint(bytes, cursor);
  const col = readVarUint(bytes, cursor);
  const seq = readVarUint(bytes, cursor);
  const visible = readBool(bytes, cursor);
  const blink = readBool(bytes, cursor);
  return { row, col, seq, visible, blink };
}

export function encodeHostFrameBinary(frame: HostFrame): Uint8Array {
  const out: number[] = [];
  switch (frame.type) {
    case 'heartbeat': {
      writeHeader(HOST_KIND_HEARTBEAT, out);
      writeVarUint(ensureSafe(frame.seq, 'seq'), out);
      writeVarUint(ensureSafe(frame.timestampMs, 'timestampMs'), out);
      break;
    }
    case 'hello': {
      writeHeader(HOST_KIND_HELLO, out);
      writeVarUint(ensureSafe(frame.subscription, 'subscription'), out);
      writeVarUint(ensureSafe(frame.maxSeq, 'maxSeq'), out);
      writeSyncConfig(frame.config, out);
      writeVarUint(ensureSafe(frame.features, 'features'), out);
      break;
    }
    case 'grid': {
      writeHeader(HOST_KIND_GRID, out);
      if (typeof frame.viewportRows === 'number') {
        writeVarUint(ensureSafe(frame.viewportRows, 'viewportRows'), out);
      }
      writeVarUint(ensureSafe(frame.cols, 'cols'), out);
      writeVarUint(ensureSafe(frame.historyRows, 'historyRows'), out);
      writeVarUint(ensureSafe(frame.baseRow, 'baseRow'), out);
      break;
    }
    case 'snapshot': {
      writeHeader(HOST_KIND_SNAPSHOT, out);
      writeVarUint(ensureSafe(frame.subscription, 'subscription'), out);
      writeLane(frame.lane, out);
      writeVarUint(ensureSafe(frame.watermark, 'watermark'), out);
      writeBool(frame.hasMore, out);
      writeUpdates(frame.updates, out);
      writeBool(Boolean(frame.cursor), out);
      if (frame.cursor) {
        writeCursorFrame(frame.cursor, out);
      }
      break;
    }
    case 'snapshot_complete': {
      writeHeader(HOST_KIND_SNAPSHOT_COMPLETE, out);
      writeVarUint(ensureSafe(frame.subscription, 'subscription'), out);
      writeLane(frame.lane, out);
      break;
    }
    case 'delta': {
      writeHeader(HOST_KIND_DELTA, out);
      writeVarUint(ensureSafe(frame.subscription, 'subscription'), out);
      writeVarUint(ensureSafe(frame.watermark, 'watermark'), out);
      writeBool(frame.hasMore, out);
      writeUpdates(frame.updates, out);
      writeBool(Boolean(frame.cursor), out);
      if (frame.cursor) {
        writeCursorFrame(frame.cursor, out);
      }
      break;
    }
    case 'history_backfill': {
      writeHeader(HOST_KIND_HISTORY_BACKFILL, out);
      writeVarUint(ensureSafe(frame.subscription, 'subscription'), out);
      writeVarUint(ensureSafe(frame.requestId, 'requestId'), out);
      writeVarUint(ensureSafe(frame.startRow, 'startRow'), out);
      writeVarUint(ensureSafe(frame.count, 'count'), out);
      writeBool(frame.more, out);
      writeUpdates(frame.updates, out);
      writeBool(Boolean(frame.cursor), out);
      if (frame.cursor) {
        writeCursorFrame(frame.cursor, out);
      }
      break;
    }
    case 'input_ack': {
      writeHeader(HOST_KIND_INPUT_ACK, out);
      writeVarUint(ensureSafe(frame.seq, 'seq'), out);
      break;
    }
    case 'cursor': {
      writeHeader(HOST_KIND_CURSOR, out);
      writeVarUint(ensureSafe(frame.subscription, 'subscription'), out);
      writeCursorFrame(frame.cursor, out);
      break;
    }
    case 'shutdown': {
      writeHeader(HOST_KIND_SHUTDOWN, out);
      break;
    }
    default: {
      const neverFrame: never = frame;
      throw new Error(`unsupported host frame ${(neverFrame as HostFrame).type}`);
    }
  }
  return Uint8Array.from(out);
}

export function decodeHostFrameBinary(input: ArrayBuffer | Uint8Array): HostFrame {
  const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
  const cursor = createCursor();
  const kind = readHeader(bytes, cursor);

  switch (kind) {
    case HOST_KIND_HEARTBEAT: {
      const seq = readVarUint(bytes, cursor);
      const timestampMs = readVarUint(bytes, cursor);
      return { type: 'heartbeat', seq, timestampMs };
    }
    case HOST_KIND_HELLO: {
      const subscription = readVarUint(bytes, cursor);
      const maxSeq = readVarUint(bytes, cursor);
      const config = readSyncConfig(bytes, cursor);
      const features = readVarUint(bytes, cursor);
      return { type: 'hello', subscription, maxSeq, config, features };
    }
    case HOST_KIND_GRID: {
      const checkpoint = cursor.value;
      const cols = readVarUint(bytes, cursor);
      const historyRows = readVarUint(bytes, cursor);
      const baseRow = readVarUint(bytes, cursor);
      if (cursor.value === bytes.length) {
        return { type: 'grid', cols, historyRows, baseRow };
      }
      cursor.value = checkpoint;
      const viewportRows = readVarUint(bytes, cursor);
      const legacyCols = readVarUint(bytes, cursor);
      const legacyHistoryRows = readVarUint(bytes, cursor);
      const legacyBaseRow = readVarUint(bytes, cursor);
      return {
        type: 'grid',
        cols: legacyCols,
        historyRows: legacyHistoryRows,
        baseRow: legacyBaseRow,
        viewportRows,
      };
    }
    case HOST_KIND_SNAPSHOT: {
      const subscription = readVarUint(bytes, cursor);
      const lane = readLane(bytes, cursor);
      const watermark = readVarUint(bytes, cursor);
      const hasMore = readBool(bytes, cursor);
      const updates = readUpdates(bytes, cursor);
      const hasCursor = readBool(bytes, cursor);
      const cursorFrame = hasCursor ? readCursorFrame(bytes, cursor) : undefined;
      return { type: 'snapshot', subscription, lane, watermark, hasMore, updates, cursor: cursorFrame };
    }
    case HOST_KIND_SNAPSHOT_COMPLETE: {
      const subscription = readVarUint(bytes, cursor);
      const lane = readLane(bytes, cursor);
      return { type: 'snapshot_complete', subscription, lane };
    }
    case HOST_KIND_DELTA: {
      const subscription = readVarUint(bytes, cursor);
      const watermark = readVarUint(bytes, cursor);
      const hasMore = readBool(bytes, cursor);
      const updates = readUpdates(bytes, cursor);
      const hasCursor = readBool(bytes, cursor);
      const cursorFrame = hasCursor ? readCursorFrame(bytes, cursor) : undefined;
      return { type: 'delta', subscription, watermark, hasMore, updates, cursor: cursorFrame };
    }
    case HOST_KIND_HISTORY_BACKFILL: {
      const subscription = readVarUint(bytes, cursor);
      const requestId = readVarUint(bytes, cursor);
      const startRow = readVarUint(bytes, cursor);
      const count = readVarUint(bytes, cursor);
      const more = readBool(bytes, cursor);
      const updates = readUpdates(bytes, cursor);
      const hasCursor = readBool(bytes, cursor);
      const cursorFrame = hasCursor ? readCursorFrame(bytes, cursor) : undefined;
      return {
        type: 'history_backfill',
        subscription,
        requestId,
        startRow,
        count,
        updates,
        more,
        cursor: cursorFrame,
      };
    }
    case HOST_KIND_INPUT_ACK: {
      const seq = readVarUint(bytes, cursor);
      return { type: 'input_ack', seq };
    }
    case HOST_KIND_CURSOR: {
      const subscription = readVarUint(bytes, cursor);
      const cursorFrame = readCursorFrame(bytes, cursor);
      return { type: 'cursor', subscription, cursor: cursorFrame };
    }
    case HOST_KIND_SHUTDOWN:
      return { type: 'shutdown' };
    default:
      throw new RangeError(`unknown host frame type: ${kind}`);
  }
}

export function encodeClientFrameBinary(frame: ClientFrame): Uint8Array {
  const out: number[] = [];
  switch (frame.type) {
    case 'input': {
      writeHeader(CLIENT_KIND_INPUT, out);
      writeVarUint(ensureSafe(frame.seq, 'seq'), out);
      writeVarUint(frame.data.length, out);
      writeBytes(frame.data, out);
      break;
    }
    case 'resize': {
      writeHeader(CLIENT_KIND_RESIZE, out);
      writeVarUint(ensureSafe(frame.cols, 'cols'), out);
      writeVarUint(ensureSafe(frame.rows, 'rows'), out);
      break;
    }
    case 'request_backfill': {
      writeHeader(CLIENT_KIND_REQUEST_BACKFILL, out);
      writeVarUint(ensureSafe(frame.subscription, 'subscription'), out);
      writeVarUint(ensureSafe(frame.requestId, 'requestId'), out);
      writeVarUint(ensureSafe(frame.startRow, 'startRow'), out);
      writeVarUint(ensureSafe(frame.count, 'count'), out);
      break;
    }
    default: {
      const neverFrame: never = frame;
      throw new Error(`unsupported client frame ${(neverFrame as ClientFrame).type}`);
    }
  }
  return Uint8Array.from(out);
}

export function decodeClientFrameBinary(input: ArrayBuffer | Uint8Array): ClientFrame {
  const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
  const cursor = createCursor();
  const kind = readHeader(bytes, cursor);

  switch (kind) {
    case CLIENT_KIND_INPUT: {
      const seq = readVarUint(bytes, cursor);
      const length = readVarUint(bytes, cursor);
      const data = readBytes(bytes, length, cursor);
      return { type: 'input', seq, data: data.slice() };
    }
    case CLIENT_KIND_RESIZE: {
      const cols = readVarUint(bytes, cursor);
      const rows = readVarUint(bytes, cursor);
      return { type: 'resize', cols, rows };
    }
    case CLIENT_KIND_REQUEST_BACKFILL: {
      const subscription = readVarUint(bytes, cursor);
      const requestId = readVarUint(bytes, cursor);
      const startRow = readVarUint(bytes, cursor);
      const count = readVarUint(bytes, cursor);
      return { type: 'request_backfill', subscription, requestId, startRow, count };
    }
    default:
      throw new RangeError(`unknown client frame type: ${kind}`);
  }
}
