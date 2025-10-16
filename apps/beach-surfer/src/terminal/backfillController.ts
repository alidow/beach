import type { ClientFrame, HostFrame } from '../protocol/types';
import type { TerminalGridSnapshot, TerminalGridStore } from './gridStore';

declare global {
  interface Window {
    __BEACH_TRACE?: boolean;
  }
}

function trace(...parts: unknown[]): void {
  if (typeof window !== 'undefined' && window.__BEACH_TRACE) {
    console.debug('[beach-trace][backfill]', ...parts);
  }
}

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
    trace('handle_frame', { type: frame.type });
    switch (frame.type) {
      case 'hello':
        this.subscriptionId = frame.subscription;
        this.pending = [];
        break;
      case 'history_backfill': {
        trace('history_backfill_frame', {
          requestId: frame.requestId,
          start: frame.startRow,
          count: frame.count,
          more: frame.more,
        });
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
    if (!this.subscriptionId) {
      return;
    }
    const now = Date.now();
    trace('maybe_request', {
      followTail,
      baseRow: snapshot.baseRow,
      viewportTop: snapshot.viewportTop,
      viewportHeight: snapshot.viewportHeight,
      totalRows: snapshot.rows.length,
      pending: this.pending.length,
      lastRequestAt: this.lastRequestAt,
    });
    if (now - this.lastRequestAt < BACKFILL_THROTTLE_MS) {
      trace('maybe_request_skipped_throttle', { elapsed: now - this.lastRequestAt });
      return;
    }

    if (followTail) {
      const tailGap = findTailGap(snapshot);
      trace('follow_tail_scan', { tailGap });
      if (tailGap) {
        const requestEnd = Math.min(tailGap.end, tailGap.start + BACKFILL_MAX_ROWS_PER_REQUEST);
        const count = requestEnd - tailGap.start;
        trace('tail_gap_found', { start: tailGap.start, end: tailGap.end, requestEnd, count });
        const isPending = this.isRangePending(tailGap.start, requestEnd);
        if (count > 0 && !isPending) {
          const requestId = this.enqueueRequest(tailGap.start, requestEnd);
          trace('tail_gap_request_sent', { requestId, start: tailGap.start, count });
          this.issueRequest(requestId, tailGap.start, count);
          this.lastRequestAt = now;
        } else {
          trace('tail_gap_request_skipped', {
            pending: isPending,
            count,
          });
        }
        return;
      }
      trace('tail_gap_missing', {});
    }

    const earliestLoaded = minLoadedRow(snapshot.rows);
    if (earliestLoaded === null) {
      trace('maybe_request_no_loaded_rows', {});
      return;
    }

    const viewportTop = snapshot.viewportTop;
    const distanceFromTop = viewportTop - earliestLoaded;
    if (distanceFromTop > BACKFILL_LOOKAHEAD_ROWS) {
      trace('maybe_request_skip_distance', { distanceFromTop });
      return;
    }

    if (earliestLoaded === 0) {
      trace('maybe_request_at_origin');
      return;
    }

    const start = Math.max(0, earliestLoaded - BACKFILL_MAX_ROWS_PER_REQUEST);
    const end = earliestLoaded;
    if (this.isRangePending(start, end)) {
      trace('maybe_request_pending_skip', { start, end });
      return;
    }

    const count = end - start;
    if (count <= 0) {
      trace('maybe_request_empty_range', { start, end });
      return;
    }

    const requestId = this.enqueueRequest(start, end);
    trace('maybe_request_sent', { requestId, start, end, count, followTail });
    this.issueRequest(requestId, start, count);
    this.lastRequestAt = now;
  }

  private enqueueRequest(start: number, end: number): number {
    const id = this.nextRequestId++;
    this.pending.push({ id, start, end, issuedAt: Date.now() });
    trace('enqueue_request', { id, start, end });
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
    trace('issue_request', { requestId, startRow, count });
    this.sendFrame(frame);
  }

  private isRangePending(start: number, end: number): boolean {
    return this.pending.some((range) => rangesOverlap(range.start, range.end, start, end));
  }

  private clearPending(start: number, end: number): void {
    trace('clear_pending', { start, end });
    this.pending = this.pending.filter((range) => !rangesOverlap(range.start, range.end, start, end));
  }

  finalizeHistoryBackfill(frame: Extract<HostFrame, { type: 'history_backfill' }>): void {
    trace('finalize_history_backfill', {
      requestId: frame.requestId,
      startRow: frame.startRow,
      count: frame.count,
      more: frame.more,
    });
    if (frame.more) {
      return;
    }
    const startRow = frame.startRow;
    const endRow = frame.startRow + frame.count;
    for (let absolute = startRow; absolute < endRow; absolute += 1) {
      const row = this.store.getRow(absolute);
      if (!row || row.kind !== 'loaded') {
        this.store.markRowMissing(absolute);
      }
    }
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


function maxLoadedRow(rows: TerminalGridSnapshot['rows']): number | null {
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    const row = rows[index];
    if (row.kind === 'loaded') {
      return row.absolute;
    }
  }
  return null;
}

function findTailGap(snapshot: TerminalGridSnapshot): { start: number; end: number } | null {
  const highest = maxLoadedRow(snapshot.rows);
  if (highest === null) {
    return null;
  }
  const scanStart = Math.max(snapshot.baseRow, highest - BACKFILL_LOOKAHEAD_ROWS);
  let gapStart: number | null = null;
  for (let absolute = scanStart; absolute <= highest; absolute += 1) {
    const index = absolute - snapshot.baseRow;
    if (index < 0 || index >= snapshot.rows.length) {
      continue;
    }
    const slot = snapshot.rows[index];
    if (!slot || slot.kind !== 'loaded') {
      if (gapStart === null) {
        gapStart = absolute;
      }
      continue;
    }
    if (gapStart !== null) {
      return { start: gapStart, end: absolute };
    }
  }
  if (gapStart !== null) {
    return { start: gapStart, end: highest + 1 };
  }
  return null;
}
