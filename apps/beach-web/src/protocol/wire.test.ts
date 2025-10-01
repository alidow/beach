import { describe, expect, it } from 'vitest';
import {
  decodeClientFrameBinary,
  decodeHostFrameBinary,
  encodeClientFrameBinary,
  encodeHostFrameBinary,
} from './wire';
import { Lane } from './types';

function roundTripHost(frame: Parameters<typeof encodeHostFrameBinary>[0]) {
  const encoded = encodeHostFrameBinary(frame);
  const decoded = decodeHostFrameBinary(encoded);
  expect(decoded).toEqual(frame);
}

function roundTripClient(frame: Parameters<typeof encodeClientFrameBinary>[0]) {
  const encoded = encodeClientFrameBinary(frame);
  const decoded = decodeClientFrameBinary(encoded);
  expect(decoded).toEqual(frame);
}

describe('wire codec', () => {
  it('round-trips heartbeat frames', () => {
    roundTripHost({ type: 'heartbeat', seq: 42, timestampMs: 123456 });
  });

  it('round-trips hello frames', () => {
    roundTripHost({
      type: 'hello',
      subscription: 9,
      maxSeq: 1024,
      config: {
        snapshotBudgets: [
          { lane: Lane.Foreground, maxUpdates: 16 },
          { lane: Lane.Recent, maxUpdates: 32 },
        ],
        deltaBudget: 128,
        heartbeatMs: 250,
        initialSnapshotLines: 64,
      },
      features: 0b101,
    });
  });

  it('decodes legacy hello frames without features', () => {
    const encoded = encodeHostFrameBinary({
      type: 'hello',
      subscription: 2,
      maxSeq: 128,
      config: {
        snapshotBudgets: [{ lane: Lane.Foreground, maxUpdates: 8 }],
        deltaBudget: 32,
        heartbeatMs: 500,
        initialSnapshotLines: 16,
      },
      features: 0,
    });
    const legacy = encoded.slice(0, -1);
    const decoded = decodeHostFrameBinary(legacy);
    expect(decoded).toEqual({
      type: 'hello',
      subscription: 2,
      maxSeq: 128,
      config: {
        snapshotBudgets: [{ lane: Lane.Foreground, maxUpdates: 8 }],
        deltaBudget: 32,
        heartbeatMs: 500,
        initialSnapshotLines: 16,
      },
      features: 0,
    });
  });

  it('round-trips snapshot frames with updates', () => {
    roundTripHost({
      type: 'snapshot',
      subscription: 1,
      lane: Lane.Foreground,
      watermark: 9001,
      hasMore: true,
      updates: [
        { type: 'style', id: 1, seq: 1, fg: 0xffffff, bg: 0x000000, attrs: 0b0010_0001 },
        { type: 'row', row: 5, seq: 2, cells: [0x4141_0001, 0x4242_0001] },
        { type: 'cell', row: 5, col: 4, seq: 3, cell: 0x4300_0001 },
        {
          type: 'row_segment',
          row: 6,
          startCol: 3,
          seq: 4,
          cells: [0x4400_0001, 0x4500_0001],
        },
        { type: 'trim', start: 2, count: 3, seq: 5 },
        { type: 'rect', rows: [7, 8], cols: [0, 10], seq: 6, cell: 0x4600_0001 },
      ],
      cursor: { row: 5, col: 6, seq: 7, visible: true, blink: false },
    });
  });

  it('round-trips history backfill frames', () => {
    roundTripHost({
      type: 'history_backfill',
      subscription: 1,
      requestId: 2,
      startRow: 100,
      count: 25,
      updates: [],
      more: false,
      cursor: { row: 101, col: 0, seq: 9, visible: false, blink: false },
    });
  });

  it('round-trips delta frames without updates', () => {
    roundTripHost({
      type: 'delta',
      subscription: 9,
      watermark: 100,
      hasMore: false,
      updates: [],
      cursor: { row: 42, col: 1, seq: 200, visible: true, blink: true },
    });
  });

  it('round-trips cursor-only frames', () => {
    roundTripHost({
      type: 'cursor',
      subscription: 5,
      cursor: { row: 10, col: 3, seq: 12, visible: false, blink: true },
    });
  });

  it('round-trips client input frames', () => {
    roundTripClient({ type: 'input', seq: 7, data: Uint8Array.from([0x1b, 0x5b, 0x41]) });
  });

  it('round-trips client resize frames', () => {
    roundTripClient({ type: 'resize', cols: 120, rows: 32 });
  });

  it('round-trips client backfill request frames', () => {
    roundTripClient({
      type: 'request_backfill',
      subscription: 1,
      requestId: 9,
      startRow: 256,
      count: 64,
    });
  });
});
