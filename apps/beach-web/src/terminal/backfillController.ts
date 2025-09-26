import type { ClientFrame, HostFrame } from '../protocol/types';
import type { TerminalGridSnapshot, TerminalGridStore } from './gridStore';

const BACKFILL_LOOKAHEAD_ROWS = 64;
const BACKFILL_MAX_ROWS_PER_REQUEST = 256;
const BACKFILL_THROTTLE_MS = 200;

interface PendingRange {
  id: number;
  start: number;
  end: number;
  issuedAt: number;
}

type SendFrameFn = (frame: ClientFrame) => void;

export class BackfillController {
  private readonly store: TerminalGridStore;
  private readonly sendFrame: SendFrameFn;
  private subscriptionId: number | null = null;
  private nextRequestId = 1;
  private pending: PendingRange[] = [];
  private lastRequestAt = 0;

  constructor(store: TerminalGridStore, sendFrame: SendFrameFn) {
    this.store = store;
    this.sendFrame = sendFrame;
  }

  handleFrame(frame: HostFrame): void {
    switch (frame.type) {
      case 'hello':
        this.subscriptionId = frame.subscription;
        this.pending = [];
        break;
      case 'history_backfill': {
        this.clearPending(frame.startRow, frame.startRow + frame.count);
        if (frame.more) {
          // immediately follow up; the store already includes the new rows so the next
          // maybeRequest call will send another range if needed.
          this.lastRequestAt = 0;
        }
        break;
      }
      case 'delta':
      case 'snapshot':
      case 'snapshot_complete':
      case 'grid':
      case 'heartbeat':
      case 'input_ack':
      case 'shutdown':
        break;
      default:
        break;
    }
  }

  maybeRequest(snapshot: TerminalGridSnapshot, followTail: boolean): void {
    if (!this.subscriptionId || followTail) {
      return;
    }
    const now = Date.now();
    if (now - this.lastRequestAt < BACKFILL_THROTTLE_MS) {
      return;
    }

    const earliestLoaded = minLoadedRow(snapshot.rows);
    if (earliestLoaded === null) {
      return;
    }

    const viewportTop = snapshot.viewportTop;
    const distanceFromTop = viewportTop - earliestLoaded;
    if (distanceFromTop > BACKFILL_LOOKAHEAD_ROWS) {
      return;
    }

    if (earliestLoaded === 0) {
      return;
    }

    const start = Math.max(0, earliestLoaded - BACKFILL_MAX_ROWS_PER_REQUEST);
    const end = earliestLoaded;
    if (this.isRangePending(start, end)) {
      return;
    }

    const count = end - start;
    if (count <= 0) {
      return;
    }

    const requestId = this.enqueueRequest(start, end);
    this.issueRequest(requestId, start, count);
    this.lastRequestAt = now;
  }

  private enqueueRequest(start: number, end: number): number {
    const id = this.nextRequestId++;
    this.pending.push({ id, start, end, issuedAt: Date.now() });
    this.store.markPendingRange(start, end);
    return id;
  }

  private issueRequest(requestId: number, startRow: number, count: number): void {
    if (!this.subscriptionId) {
      return;
    }
    const frame: ClientFrame = {
      type: 'request_backfill',
      subscription: this.subscriptionId,
      requestId,
      startRow,
      count,
    };
    this.sendFrame(frame);
  }

  private isRangePending(start: number, end: number): boolean {
    return this.pending.some((range) => rangesOverlap(range.start, range.end, start, end));
  }

  private clearPending(start: number, end: number): void {
    this.pending = this.pending.filter((range) => !rangesOverlap(range.start, range.end, start, end));
  }
}

function rangesOverlap(aStart: number, aEnd: number, bStart: number, bEnd: number): boolean {
  return aStart < bEnd && bStart < aEnd;
}

function minLoadedRow(rows: TerminalGridSnapshot['rows']): number | null {
  let min: number | null = null;
  for (const row of rows) {
    if (row.kind !== 'loaded') {
      continue;
    }
    if (min === null || row.absolute < min) {
      min = row.absolute;
    }
  }
  return min;
}
