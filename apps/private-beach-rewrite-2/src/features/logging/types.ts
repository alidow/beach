'use client';

export type ConnectionLogLevel = 'info' | 'warn' | 'error';

export type ConnectionTimelineScope = 'viewer' | 'host' | 'connector' | 'generic';

export type ConnectionLogContext = {
  tileId?: string | null;
  sessionId?: string | null;
  privateBeachId?: string | null;
  managerUrl?: string | null;
  beachUrl?: string | null;
  timelineScope?: ConnectionTimelineScope;
  timelineGroupId?: string | null;
  timelineGroupLabel?: string | null;
  timelineTrackId?: string | null;
  timelineTrackLabel?: string | null;
  controllerSessionId?: string | null;
  childSessionId?: string | null;
};

export type ConnectionLogDetail = Record<string, unknown> | null | undefined;
