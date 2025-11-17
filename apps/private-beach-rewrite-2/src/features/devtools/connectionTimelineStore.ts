'use client';

import { useSyncExternalStore } from 'react';
import type { ConnectionLogContext, ConnectionLogDetail, ConnectionLogLevel } from '@/features/logging/types';

export type ConnectionTimelineEvent = {
  id: string;
  timestamp: number;
  step: string;
  level: ConnectionLogLevel;
  context: ConnectionLogContext;
  detail: ConnectionLogDetail;
};

export type ConnectionTimelineRecord = {
  key: string;
  label: string;
  context: ConnectionLogContext;
  events: ConnectionTimelineEvent[];
  lastTimestamp: number;
};

type TimelineBucket = {
  context: ConnectionLogContext;
  events: ConnectionTimelineEvent[];
};

type Listener = () => void;

const MAX_EVENTS_PER_KEY = 120;
const buckets = new Map<string, TimelineBucket>();
let cachedSnapshot: ConnectionTimelineRecord[] | null = null;
const listeners = new Set<Listener>();

function emit() {
  listeners.forEach((listener) => {
    try {
      listener();
    } catch (error) {
      console.error('[connection-timeline-store] listener error', error);
    }
  });
}

function deriveKey(context: ConnectionLogContext): string {
  if (context.tileId && context.tileId.length > 0) {
    return `tile:${context.tileId}`;
  }
  if (context.sessionId && context.sessionId.length > 0) {
    return `session:${context.sessionId}`;
  }
  if (context.privateBeachId && context.privateBeachId.length > 0) {
    return `beach:${context.privateBeachId}`;
  }
  return 'global';
}

function deriveLabel(context: ConnectionLogContext, key: string): string {
  if (context.tileId) {
    return context.tileId;
  }
  if (context.sessionId) {
    return context.sessionId;
  }
  if (context.privateBeachId) {
    return `beach:${context.privateBeachId}`;
  }
  return key;
}

export function recordConnectionTimelineEvent(
  step: string,
  context: ConnectionLogContext,
  detail: ConnectionLogDetail,
  level: ConnectionLogLevel,
) {
  console.info(
    '[devtools][timeline]',
    step,
    {
      tileId: context.tileId ?? null,
      sessionId: context.sessionId ?? null,
      privateBeachId: context.privateBeachId ?? null,
      level,
      detail,
    },
  );
  const key = deriveKey(context);
  const bucket = buckets.get(key) ?? {
    context,
    events: [],
  };
  bucket.context = {
    ...bucket.context,
    ...context,
  };
  const entry: ConnectionTimelineEvent = {
    id: `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`,
    timestamp: Date.now(),
    step,
    level,
    context,
    detail,
  };
  const nextEvents = bucket.events.concat(entry);
  if (nextEvents.length > MAX_EVENTS_PER_KEY) {
    nextEvents.splice(0, nextEvents.length - MAX_EVENTS_PER_KEY);
  }
  bucket.events = nextEvents;
  buckets.set(key, bucket);
  cachedSnapshot = null;
  emit();
}

function getSnapshot(): ConnectionTimelineRecord[] {
  if (cachedSnapshot) {
    return cachedSnapshot;
  }
  cachedSnapshot = Array.from(buckets.entries())
    .map(([key, bucket]) => ({
      key,
      label: deriveLabel(bucket.context, key),
      context: bucket.context,
      events: bucket.events,
      lastTimestamp: bucket.events[bucket.events.length - 1]?.timestamp ?? 0,
    }))
    .sort((a, b) => b.lastTimestamp - a.lastTimestamp);
  return cachedSnapshot;
}

export function useConnectionTimelines(): ConnectionTimelineRecord[] {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    getSnapshot,
    () => [],
  );
}

export function clearConnectionTimelines() {
  buckets.clear();
  cachedSnapshot = null;
  emit();
}
