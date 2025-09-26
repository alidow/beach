export const PROTOCOL_VERSION = 1;

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
    }
  | {
      type: 'grid';
      viewportRows: number;
      cols: number;
      historyRows: number;
      baseRow: number;
    }
  | {
      type: 'snapshot';
      subscription: number;
      lane: Lane;
      watermark: number;
      hasMore: boolean;
      updates: Update[];
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
    }
  | {
      type: 'history_backfill';
      subscription: number;
      requestId: number;
      startRow: number;
      count: number;
      updates: Update[];
      more: boolean;
    }
  | {
      type: 'input_ack';
      seq: number;
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
