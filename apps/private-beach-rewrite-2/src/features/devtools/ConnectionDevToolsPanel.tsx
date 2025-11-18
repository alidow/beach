'use client';

import { X } from 'lucide-react';
import { useMemo } from 'react';
import { cn } from '@/lib/cn';
import {
  useConnectionTimelines,
  type ConnectionTimelineRecord,
  type ConnectionTimelineTrack,
} from './connectionTimelineStore';

type DevToolsPanelProps = {
  open: boolean;
  onClose?: () => void;
};

const STEP_DESCRIPTIONS: Record<string, string> = {
  'handshake:start': 'Initial connection',
  'handshake:renew-start': 'Lease refresh started',
  'handshake:success': 'Handshake complete',
  'handshake:error': 'Handshake failed',
  'handshake:auto-error': 'Auto-connect failed',
  'hint:sent': 'Beach Manager hint received',
  'hint:error': 'Manager hint error',
  'slow-path:ready': 'Slow path ready',
  'session:detected': 'Session detected',
  'session:cleared': 'Session cleared',
  'fast-path:attempt': 'Attempting fast-path',
  'fast-path:success': 'Fast-path connected',
  'fast-path:reconnect-start': 'Reconnecting fast-path',
  'fast-path:reconnect-success': 'Fast-path reconnected',
  'fast-path:reconnect-error': 'Fast-path reconnect failed',
  'fast-path:error': 'Fast-path failure',
  'fast-path:disconnect': 'Fast-path disconnected',
  'fast-path:idle': 'Connection idle',
  'host.transport:connected': 'Host transport connected',
  'host.transport:fast-path-ready': 'Fast-path data channel ready',
  'host.transport:fallback': 'Manager switched to fallback',
  'host.transport:ack-stall': 'Ack stall detected',
  'host.transport:error': 'Host transport error',
  'host.transport:closed': 'Host transport closed',
  'host.transport:stopped': 'Host transport stopped',
  'connector.action:queued': 'Actions queued',
  'connector.action:forwarded': 'Action forwarded',
  'connector.action:ack': 'Action acknowledged',
  'connector.action:rejected': 'Action rejected',
  'connector.child:update': 'Child update',
  'connector.transport:update': 'Connector transport update',
  'viewer.webrtc:connection-state': 'Viewer peer connection',
  'viewer.webrtc:ice-state': 'Viewer ICE state',
  'viewer.webrtc:signaling-state': 'Viewer signaling state',
};

function formatTimestamp(timestamp: number): string {
  const date = new Date(timestamp);
  return date.toLocaleTimeString([], { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

function describeStep(step: string): string {
  return STEP_DESCRIPTIONS[step] ?? step;
}

function describeDetail(detail: Record<string, unknown> | null | undefined): string | null {
  if (!detail) {
    return null;
  }
  if (typeof detail.state === 'string' && detail.state.trim().length > 0) {
    return detail.state;
  }
  if (typeof detail.reason === 'string' && detail.reason.trim().length > 0) {
    return detail.reason;
  }
  if (typeof detail.nextStatus === 'string' && detail.nextStatus.trim().length > 0) {
    return detail.nextStatus;
  }
  if (typeof detail.transport === 'string' && detail.transport.trim().length > 0) {
    return detail.transport;
  }
  if (typeof detail.actionId === 'string' && detail.actionId.trim().length > 0) {
    return `action ${detail.actionId}`;
  }
  if (typeof detail.count === 'number' && detail.count > 0) {
    return `${detail.count} event${detail.count === 1 ? '' : 's'}`;
  }
  if (typeof detail.error === 'string') {
    return detail.error;
  }
  if (detail.error && typeof detail.error === 'object' && 'message' in detail.error) {
    return String((detail.error as Record<string, unknown>).message);
  }
  return null;
}

const TRACK_KIND_ORDER: Record<string, number> = {
  viewer: 0,
  host: 1,
  connector: 2,
  generic: 3,
};

function deriveTrackStatus(
  track: ConnectionTimelineTrack,
): { label: string; tone: 'ok' | 'warn' | 'error' | 'idle' } {
  const lastEvent = track.events[track.events.length - 1];
  if (!lastEvent) {
    return { label: 'Idle', tone: 'idle' };
  }
  const step = lastEvent.step;
  switch (track.kind) {
    case 'viewer':
      switch (step) {
        case 'fast-path:success':
        case 'fast-path:reconnect-success':
        case 'slow-path:ready':
          return { label: 'Connected', tone: 'ok' };
        case 'fast-path:attempt':
        case 'handshake:start':
          return { label: 'Connecting', tone: 'idle' };
        case 'fast-path:reconnect-start':
          return { label: 'Reconnecting', tone: 'warn' };
        case 'fast-path:error':
        case 'fast-path:reconnect-error':
        case 'handshake:error':
        case 'handshake:auto-error':
          return { label: 'Error', tone: 'error' };
        case 'fast-path:disconnect':
          return { label: 'Disconnected', tone: 'warn' };
        default:
          return { label: 'Active', tone: 'idle' };
      }
    case 'host':
      if (step === 'host.transport:connected' || step === 'host.transport:fast-path-ready') {
        return { label: 'Connected', tone: 'ok' };
      }
      if (step.includes('fallback') || step.includes('ack-stall')) {
        return { label: 'Fallback', tone: 'warn' };
      }
      if (step.includes('error') || step.includes('closed')) {
        return { label: 'Error', tone: 'error' };
      }
      return { label: 'Active', tone: 'idle' };
    case 'connector':
      if (step.includes('ack')) {
        return { label: 'Acked', tone: 'ok' };
      }
      if (step.includes('queued')) {
        return { label: 'Queued', tone: 'idle' };
      }
      if (step.includes('rejected')) {
        return { label: 'Rejected', tone: 'error' };
      }
      if (step.includes('update')) {
        return { label: 'Updating', tone: 'ok' };
      }
      return { label: 'Active', tone: 'idle' };
    default:
      return { label: 'Active', tone: 'idle' };
  }
}

export function ConnectionDevToolsPanel({ open, onClose }: DevToolsPanelProps) {
  const timelines = useConnectionTimelines();
  const [sessionGroups, connectorGroups] = useMemo(() => {
    const sessions: ConnectionTimelineRecord[] = [];
    const connectors: ConnectionTimelineRecord[] = [];
    timelines.forEach((group) => {
      if (group.kind === 'connector') {
        connectors.push(group);
      } else {
        sessions.push(group);
      }
    });
    return [sessions, connectors];
  }, [timelines]);

  if (!open) {
    return null;
  }

  return (
    <aside className="pointer-events-auto absolute inset-y-0 right-0 z-40 flex w-[340px] flex-col border-l border-border bg-background/95 shadow-2xl backdrop-blur">
      <div className="flex items-center justify-between border-b border-border px-4 py-3">
        <div>
          <p className="text-sm font-semibold text-foreground">Dev Tools</p>
          <p className="text-xs text-muted-foreground">Connection diagnostics</p>
        </div>
        <button
          type="button"
          onClick={onClose}
          className="inline-flex h-8 w-8 items-center justify-center rounded-lg bg-muted text-muted-foreground transition hover:text-foreground"
          aria-label="Close dev tools"
        >
          <X className="h-4 w-4" />
        </button>
      </div>
      <div className="flex-1 overflow-y-auto px-4 py-3">
        {sessionGroups.length === 0 && connectorGroups.length === 0 ? (
          <p className="text-xs text-muted-foreground">Connection events will appear here as you interact with tiles.</p>
        ) : (
          <div className="space-y-4">
            {sessionGroups.map((group) => (
              <TimelineGroupSection key={group.key} group={group} />
            ))}
            {connectorGroups.length > 0 ? (
              <div className="space-y-2">
                <p className="text-xs font-semibold uppercase text-muted-foreground">Connectors</p>
                {connectorGroups.map((group) => (
                  <TimelineGroupSection key={group.key} group={group} />
                ))}
              </div>
            ) : null}
          </div>
        )}
      </div>
    </aside>
  );
}

function TimelineGroupSection({ group }: { group: ConnectionTimelineRecord }) {
  const orderedTracks = group.tracks
    .slice()
    .sort(
      (a, b) =>
        (TRACK_KIND_ORDER[a.kind] ?? 99) - (TRACK_KIND_ORDER[b.kind] ?? 99) ||
        b.lastTimestamp - a.lastTimestamp,
    );
  return (
    <section className="rounded-lg border border-border bg-background/40 p-3">
      <div className="flex items-center justify-between text-xs text-muted-foreground">
        <div className="font-mono text-[11px] text-foreground">{group.label}</div>
        {orderedTracks.length > 0 ? (
          <span className="text-[10px] uppercase text-muted-foreground/70">{group.kind === 'connector' ? 'Connector' : 'Session'}</span>
        ) : null}
      </div>
      <div className="mt-2 space-y-2">
        {orderedTracks.map((track) => (
          <TrackTimeline key={track.key} track={track} />
        ))}
      </div>
    </section>
  );
}

function TrackTimeline({ track }: { track: ConnectionTimelineTrack }) {
  const status = deriveTrackStatus(track);
  return (
    <div className="rounded-md border border-border/60 bg-background/60 p-2">
      <div className="flex items-center justify-between text-xs text-muted-foreground">
        <span className="font-semibold text-foreground">{track.label}</span>
        <span
          className={cn(
            'inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide',
            status.tone === 'ok' && 'bg-emerald-100 text-emerald-900 dark:bg-emerald-900/30 dark:text-emerald-200',
            status.tone === 'warn' && 'bg-amber-100 text-amber-900 dark:bg-amber-900/30 dark:text-amber-100',
            status.tone === 'error' && 'bg-rose-100 text-rose-900 dark:bg-rose-900/30 dark:text-rose-100',
            status.tone === 'idle' && 'bg-muted text-foreground',
          )}
        >
          {status.label}
        </span>
      </div>
      <ol className="mt-2 max-h-40 space-y-1 overflow-y-auto pr-1 text-xs text-muted-foreground">
        {track.events.map((event) => {
          const detailText = describeDetail(event.detail);
          return (
            <li key={event.id}>
              <span className="font-mono text-[10px] text-muted-foreground/80">{formatTimestamp(event.timestamp)}</span>
              <span className="ml-2 text-foreground">{describeStep(event.step)}</span>
              {detailText ? <span className="ml-1 text-muted-foreground/70">({detailText})</span> : null}
              {event.detail && event.detail.durationMs ? (
                <span className="ml-1 text-muted-foreground/60">{Math.round(Number(event.detail.durationMs))}ms</span>
              ) : null}
            </li>
          );
        })}
      </ol>
    </div>
  );
}
