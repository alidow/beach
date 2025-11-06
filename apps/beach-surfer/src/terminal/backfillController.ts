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

function info(label: string, payload?: unknown): void {
  if (typeof window !== 'undefined' && window.__BEACH_TRACE) {
    if (payload === undefined) {
      console.info('[beach-trace][backfill]', label);
    } else {
      console.info('[beach-trace][backfill]', label, payload);
    }
  }
}

const BACKFILL_LOOKAHEAD_ROWS = 64;
const BACKFILL_MAX_ROWS_PER_REQUEST = 256;
const BACKFILL_THROTTLE_MS = 200;
const FORCED_FOLLOW_TAIL_RESTORE_SLACK = 2;

interface PendingRange {
  id: number;
  start: number;
  end: number;
  issuedAt: number;
}

type SendFrameFn = (frame: ClientFrame) => void;

type FollowTailIntentPhase = 'hydrating' | 'follow_tail' | 'manual_scrollback' | 'catching_up';

interface TailRequestContext {
  nearBottom: boolean;
  followTailDesired: boolean;
  phase: FollowTailIntentPhase;
  tailPaddingRows: number;
}

export class BackfillController {
  private readonly store: TerminalGridStore;
  private readonly sendFrame: SendFrameFn;
  private subscriptionId: number | null = null;
  private nextRequestId = 1;
  private pending: PendingRange[] = [];
  private lastRequestAt = 0;
  private forcedFollowTail = false;
  private restoreFollowTail = false;

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

  maybeRequest(snapshot: TerminalGridSnapshot, context: TailRequestContext): void {
    if (!this.subscriptionId) {
      return;
    }
    const now = Date.now();
    const snapshotFollowTail = snapshot.followTail;
    const effectiveFollowTail =
      context.followTailDesired &&
      (snapshotFollowTail ||
        context.nearBottom ||
        context.phase === 'catching_up' ||
        context.tailPaddingRows > 0);
    const highestLoaded = maxLoadedRow(snapshot.rows);
    const highestTracked = snapshot.baseRow + snapshot.rows.length - 1;
    const viewportBottom = snapshot.viewportHeight > 0
      ? snapshot.viewportTop + snapshot.viewportHeight - 1
      : snapshot.viewportTop;
    trace('maybe_request', {
      followTailDesired: context.followTailDesired,
      followTailPhase: context.phase,
      nearBottom: context.nearBottom,
      tailPaddingRows: context.tailPaddingRows,
      snapshotFollowTail,
      effectiveFollowTail,
      baseRow: snapshot.baseRow,
      viewportTop: snapshot.viewportTop,
      viewportHeight: snapshot.viewportHeight,
      viewportBottom,
      highestLoaded,
      highestTracked,
      totalRows: snapshot.rows.length,
      pending: this.pending.length,
      lastRequestAt: this.lastRequestAt,
    });
    info('maybe_request', {
      followTailDesired: context.followTailDesired,
      followTailPhase: context.phase,
      nearBottom: context.nearBottom,
      tailPaddingRows: context.tailPaddingRows,
      snapshotFollowTail,
      effectiveFollowTail,
      viewportTop: snapshot.viewportTop,
      viewportBottom,
      highestLoaded,
      highestTracked,
      pendingRequests: this.pending.length,
    });

    if (this.forcedFollowTail) {
      if (highestLoaded !== null && viewportBottom <= highestLoaded) {
        const distanceToTail = highestLoaded - viewportBottom;
        const shouldRestore = this.restoreFollowTail
          && snapshotFollowTail
          && distanceToTail <= FORCED_FOLLOW_TAIL_RESTORE_SLACK;
        trace('follow_tail_restore_ready', {
          viewportBottom,
          highestLoaded,
          restoreFollowTail: this.restoreFollowTail,
          snapshotFollowTail,
          distanceToTail,
          slack: FORCED_FOLLOW_TAIL_RESTORE_SLACK,
          shouldRestore,
        });
        if (shouldRestore) {
          this.store.setFollowTail(false);
          info('follow_tail_restored', {
            viewportBottom,
            highestLoaded,
            distanceToTail,
            slack: FORCED_FOLLOW_TAIL_RESTORE_SLACK,
          });
        }
        this.forcedFollowTail = false;
        this.restoreFollowTail = false;
      } else {
        const distanceToTail = highestLoaded !== null ? highestLoaded - viewportBottom : null;
        trace('follow_tail_restore_pending', {
          viewportBottom,
          highestLoaded,
          distanceToTail,
          slack: FORCED_FOLLOW_TAIL_RESTORE_SLACK,
        });
      }
    }
    if (now - this.lastRequestAt < BACKFILL_THROTTLE_MS) {
      trace('maybe_request_skipped_throttle', { elapsed: now - this.lastRequestAt });
      return;
    }

    if (effectiveFollowTail) {
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

    if (!snapshotFollowTail && snapshot.viewportHeight > 0) {
      const tailGap = findTailGap(snapshot);
      if (tailGap) {
        trace('viewport_gap_scan', { tailGap, viewportBottom });
        const viewportLimitedEnd = viewportBottom >= tailGap.start
          ? Math.min(tailGap.end, viewportBottom + 1)
          : tailGap.end;
        const requestEnd = Math.min(
          tailGap.start + BACKFILL_MAX_ROWS_PER_REQUEST,
          viewportLimitedEnd,
          tailGap.end,
        );
        if (requestEnd > tailGap.start) {
          const count = Math.max(0, requestEnd - tailGap.start);
          const pending = this.isRangePending(tailGap.start, requestEnd);
          if (count > 0 && !pending) {
            const requestId = this.enqueueRequest(tailGap.start, requestEnd);
            trace('viewport_gap_request_sent', {
              requestId,
              start: tailGap.start,
              end: requestEnd,
              count,
              reason: viewportBottom >= tailGap.start ? 'internal-gap' : 'tail-gap-offscreen',
            });
            this.issueRequest(requestId, tailGap.start, count);
            this.lastRequestAt = now;
            if (!this.forcedFollowTail && snapshotFollowTail && context.followTailDesired) {
              this.store.setFollowTail(true);
              this.forcedFollowTail = true;
              this.restoreFollowTail = !snapshotFollowTail;
              trace('follow_tail_forced', { reason: 'internal-gap' });
              info('follow_tail_forced', {
                reason: 'internal-gap',
                viewportBottom,
                start: tailGap.start,
                end: requestEnd,
                restoreFollowTail: this.restoreFollowTail,
              });
            }
            return;
          }
          trace('viewport_gap_request_skipped', {
            start: tailGap.start,
            end: requestEnd,
            count,
            pending,
          });
          return;
        }
        trace('viewport_gap_request_ignored', {
          start: tailGap.start,
          end: tailGap.end,
          viewportLimitedEnd,
        });
        return;
      } else {
        trace('viewport_gap_none', {});
      }

      if (highestLoaded !== null && viewportBottom > highestLoaded) {
        const gapStart = highestLoaded + 1;
        const gapEnd = Math.min(
          Math.max(gapStart, viewportBottom + 1),
          gapStart + BACKFILL_MAX_ROWS_PER_REQUEST,
          highestTracked + 1,
        );
        const count = Math.max(0, gapEnd - gapStart);
        const pending = this.isRangePending(gapStart, gapEnd);
        trace('viewport_tail_extension', {
          gapStart,
          gapEnd,
          count,
          pending,
          highestLoaded,
          highestTracked,
          viewportBottom,
        });
        if (count > 0 && !pending) {
          const requestId = this.enqueueRequest(gapStart, gapEnd);
          trace('viewport_tail_request_sent', {
            requestId,
            start: gapStart,
            end: gapEnd,
            count,
            reason: 'viewport-extension',
          });
          this.issueRequest(requestId, gapStart, count);
          this.lastRequestAt = now;
          if (!this.forcedFollowTail && snapshotFollowTail && context.followTailDesired) {
            this.store.setFollowTail(true);
            this.forcedFollowTail = true;
            this.restoreFollowTail = !snapshotFollowTail;
            trace('follow_tail_forced', { reason: 'viewport-extension' });
            info('follow_tail_forced', {
              reason: 'viewport-extension',
              gapStart,
              gapEnd,
              viewportBottom,
              restoreFollowTail: this.restoreFollowTail,
            });
          }
          return;
        }
      }
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
    trace('maybe_request_sent', { requestId, start, end, count, followTail: effectiveFollowTail });
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
  const highestLoaded = maxLoadedRow(snapshot.rows);
  const trackedEndExclusive = snapshot.baseRow + snapshot.rows.length;
  const scanEndExclusive = Math.max(
    trackedEndExclusive,
    highestLoaded !== null ? highestLoaded + 1 : snapshot.baseRow,
  );
  const scanStart = Math.max(snapshot.baseRow, scanEndExclusive - BACKFILL_LOOKAHEAD_ROWS);
  if (scanEndExclusive <= scanStart) {
    return null;
  }
  let gapStart: number | null = null;
  for (let absolute = scanStart; absolute < scanEndExclusive; absolute += 1) {
    const index = absolute - snapshot.baseRow;
    if (index < 0 || index >= snapshot.rows.length) {
      continue;
    }
    const slot = snapshot.rows[index];
    if (!slot || slot.kind !== 'loaded' || slot.latestSeq === 0) {
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
    return { start: gapStart, end: scanEndExclusive };
  }
  return null;
}
