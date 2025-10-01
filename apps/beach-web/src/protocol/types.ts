export const PROTOCOL_VERSION = 2;
export const FEATURE_CURSOR_SYNC = 1 << 0;

export enum Lane {
  Foreground = 0,
  Recent = 1,
  History = 2,
}

export interface LaneBudgetFrame {
  lane: Lane;
  maxUpdates: number;
}

export interface SyncConfigFrame {
  snapshotBudgets: LaneBudgetFrame[];
  deltaBudget: number;
  heartbeatMs: number;
  initialSnapshotLines: number;
}

export interface CursorFrame {
  row: number;
  col: number;
  seq: number;
  visible: boolean;
  blink: boolean;
}

export type Update =
  | {
      type: 'cell';
      row: number;
      col: number;
      seq: number;
      cell: number;
    }
  | {
      type: 'rect';
      rows: [number, number];
      cols: [number, number];
      seq: number;
      cell: number;
    }
  | {
      type: 'row';
      row: number;
      seq: number;
      cells: number[];
    }
  | {
      type: 'row_segment';
      row: number;
      startCol: number;
      seq: number;
      cells: number[];
    }
  | {
      type: 'trim';
      start: number;
      count: number;
      seq: number;
    }
  | {
      type: 'style';
      id: number;
      seq: number;
      fg: number;
      bg: number;
      attrs: number;
    };

export type HostFrame =
  | {
      type: 'heartbeat';
      seq: number;
      timestampMs: number;
    }
  | {
      type: 'hello';
      subscription: number;
      maxSeq: number;
      config: SyncConfigFrame;
      features: number;
    }
  | {
      type: 'grid';
      cols: number;
      historyRows: number;
      baseRow: number;
      viewportRows?: number;
    }
  | {
      type: 'snapshot';
      subscription: number;
      lane: Lane;
      watermark: number;
      hasMore: boolean;
      updates: Update[];
      cursor?: CursorFrame;
    }
  | {
      type: 'snapshot_complete';
      subscription: number;
      lane: Lane;
    }
  | {
      type: 'delta';
      subscription: number;
      watermark: number;
      hasMore: boolean;
      updates: Update[];
      cursor?: CursorFrame;
    }
  | {
      type: 'history_backfill';
      subscription: number;
      requestId: number;
      startRow: number;
      count: number;
      updates: Update[];
      more: boolean;
      cursor?: CursorFrame;
    }
  | {
      type: 'input_ack';
      seq: number;
    }
  | {
      type: 'cursor';
      subscription: number;
      cursor: CursorFrame;
    }
  | {
      type: 'shutdown';
    };

export type ClientFrame =
  | {
      type: 'input';
      seq: number;
      data: Uint8Array;
    }
  | {
      type: 'resize';
      cols: number;
      rows: number;
    }
  | {
      type: 'request_backfill';
      subscription: number;
      requestId: number;
      startRow: number;
      count: number;
    };

export type ClientFrameKind = ClientFrame['type'];
export type HostFrameKind = HostFrame['type'];
