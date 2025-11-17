'use client';

export type ConnectionLogLevel = 'info' | 'warn' | 'error';

export type ConnectionLogContext = {
  tileId?: string | null;
  sessionId?: string | null;
  privateBeachId?: string | null;
  managerUrl?: string | null;
  beachUrl?: string | null;
};

export type ConnectionLogDetail = Record<string, unknown> | null | undefined;
