'use client';

import { useSyncExternalStore } from 'react';
import type {
  ConnectionLogContext,
  ConnectionLogDetail,
  ConnectionLogLevel,
  ConnectionTimelineScope,
} from '@/features/logging/types';

export type ConnectionTimelineEvent = {
  id: string;
  timestamp: number;
  step: string;
  level: ConnectionLogLevel;
  context: ConnectionLogContext;
  detail: ConnectionLogDetail;
};

export type ConnectionTimelineTrack = {
  key: string;
  label: string;
  kind: ConnectionTimelineScope;
  groupKey: string;
  groupLabel: string;
  context: ConnectionLogContext;
  events: ConnectionTimelineEvent[];
  lastTimestamp: number;
};

export type ConnectionTimelineGroupKind = 'session' | 'connector' | 'global';

export type ConnectionTimelineRecord = {
  key: string;
  label: string;
  kind: ConnectionTimelineGroupKind;
  context: ConnectionLogContext;
  tracks: ConnectionTimelineTrack[];
  lastTimestamp: number;
};

type Listener = () => void;

const MAX_EVENTS_PER_KEY = 120;
const tracks = new Map<string, ConnectionTimelineTrack>();
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

function deriveTimelineScope(context: ConnectionLogContext): ConnectionTimelineScope {
  if (context.timelineScope) {
    return context.timelineScope;
  }
  if (context.controllerSessionId || context.childSessionId) {
    return 'connector';
  }
  return 'viewer';
}

function deriveGroupKey(context: ConnectionLogContext, scope: ConnectionTimelineScope): string {
  if (context.timelineGroupId && context.timelineGroupId.length > 0) {
    return context.timelineGroupId;
  }
  if (scope === 'connector') {
    const controller = context.controllerSessionId ?? 'controller:unknown';
    const child = context.childSessionId ?? context.sessionId ?? 'child:unknown';
    return `connector:${controller}:${child}`;
  }
  if (context.sessionId && context.sessionId.length > 0) {
    return `session:${context.sessionId}`;
  }
  if (context.tileId && context.tileId.length > 0) {
    return `tile:${context.tileId}`;
  }
  if (context.privateBeachId && context.privateBeachId.length > 0) {
    return `beach:${context.privateBeachId}`;
  }
  return 'global';
}

function deriveGroupLabel(
  context: ConnectionLogContext,
  scope: ConnectionTimelineScope,
  key: string,
): string {
  if (context.timelineGroupLabel && context.timelineGroupLabel.length > 0) {
    return context.timelineGroupLabel;
  }
  if (scope === 'connector') {
    const controller = context.controllerSessionId ?? 'controller';
    const child = context.childSessionId ?? context.sessionId ?? 'child';
    return `${controller} → ${child}`;
  }
  if (context.sessionId) {
    return context.sessionId;
  }
  if (context.tileId) {
    return context.tileId;
  }
  if (context.privateBeachId) {
    return `beach:${context.privateBeachId}`;
  }
  if (key.startsWith('connector:')) {
    return key.replace('connector:', '');
  }
  return 'Global';
}

function deriveTrackKey(
  groupKey: string,
  scope: ConnectionTimelineScope,
  context: ConnectionLogContext,
): string {
  if (context.timelineTrackId && context.timelineTrackId.length > 0) {
    return context.timelineTrackId;
  }
  switch (scope) {
    case 'viewer':
      return `${groupKey}:viewer`;
    case 'host':
      return `${groupKey}:host`;
    default:
      return groupKey;
  }
}

function deriveTrackLabel(scope: ConnectionTimelineScope, context: ConnectionLogContext): string {
  if (context.timelineTrackLabel && context.timelineTrackLabel.length > 0) {
    return context.timelineTrackLabel;
  }
  switch (scope) {
    case 'viewer':
      return 'Viewer↔Manager';
    case 'host':
      return 'Manager↔Host';
    case 'connector':
      return 'Connector';
    default:
      return 'Events';
  }
}

function deriveGroupKind(
  scope: ConnectionTimelineScope,
  groupKey: string,
): ConnectionTimelineGroupKind {
  if (scope === 'connector' || groupKey.startsWith('connector:')) {
    return 'connector';
  }
  if (groupKey === 'global') {
    return 'global';
  }
  return 'session';
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
  const scope = deriveTimelineScope(context);
  const groupKey = deriveGroupKey(context, scope);
  const groupLabel = deriveGroupLabel(context, scope, groupKey);
  const trackKey = deriveTrackKey(groupKey, scope, context);
  const trackLabel = deriveTrackLabel(scope, context);
  const currentTrack = tracks.get(trackKey) ?? {
    key: trackKey,
    label: trackLabel,
    kind: scope,
    groupKey,
    groupLabel,
    context,
    events: [],
    lastTimestamp: 0,
  };
  currentTrack.context = {
    ...currentTrack.context,
    ...context,
  };
  currentTrack.label = context.timelineTrackLabel ?? currentTrack.label;
  currentTrack.groupLabel =
    context.timelineGroupLabel && context.timelineGroupLabel.length > 0
      ? context.timelineGroupLabel
      : currentTrack.groupLabel;
  const entry: ConnectionTimelineEvent = {
    id: `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`,
    timestamp: Date.now(),
    step,
    level,
    context,
    detail,
  };
  const nextEvents = currentTrack.events.concat(entry);
  if (nextEvents.length > MAX_EVENTS_PER_KEY) {
    nextEvents.splice(0, nextEvents.length - MAX_EVENTS_PER_KEY);
  }
  currentTrack.events = nextEvents;
  currentTrack.lastTimestamp = entry.timestamp;
  tracks.set(trackKey, currentTrack);
  cachedSnapshot = null;
  emit();
}

function getSnapshot(): ConnectionTimelineRecord[] {
  if (cachedSnapshot) {
    return cachedSnapshot;
  }
  const grouped = new Map<string, ConnectionTimelineRecord>();
  for (const track of tracks.values()) {
    const kind = deriveGroupKind(track.kind, track.groupKey);
    const existing = grouped.get(track.groupKey) ?? {
      key: track.groupKey,
      label: track.groupLabel,
      kind,
      context: track.context,
      tracks: [],
      lastTimestamp: 0,
    };
    existing.label = track.groupLabel;
    existing.context = {
      ...existing.context,
      ...track.context,
    };
    existing.tracks = existing.tracks.concat(track);
    existing.lastTimestamp = Math.max(existing.lastTimestamp, track.lastTimestamp);
    grouped.set(track.groupKey, existing);
  }
  cachedSnapshot = Array.from(grouped.values())
    .map((group) => ({
      ...group,
      tracks: group.tracks
        .slice()
        .sort((a, b) => b.lastTimestamp - a.lastTimestamp || a.label.localeCompare(b.label)),
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
  tracks.clear();
  cachedSnapshot = null;
  emit();
}
