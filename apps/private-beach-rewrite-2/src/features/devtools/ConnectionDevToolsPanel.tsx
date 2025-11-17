'use client';

import { X } from 'lucide-react';
import { useMemo } from 'react';
import { cn } from '@/lib/cn';
import { useConnectionTimelines } from './connectionTimelineStore';

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
  if (typeof detail.reason === 'string' && detail.reason.trim().length > 0) {
    return detail.reason;
  }
  if (typeof detail.nextStatus === 'string' && detail.nextStatus.trim().length > 0) {
    return detail.nextStatus;
  }
  if (typeof detail.error === 'string') {
    return detail.error;
  }
  if (detail.error && typeof detail.error === 'object' && 'message' in detail.error) {
    return String((detail.error as Record<string, unknown>).message);
  }
  return null;
}

function deriveStatus(step: string): { label: string; tone: 'ok' | 'warn' | 'error' | 'idle' } {
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
      return { label: 'Idle', tone: 'idle' };
  }
}

export function ConnectionDevToolsPanel({ open, onClose }: DevToolsPanelProps) {
  const timelines = useConnectionTimelines();
  const summary = useMemo(
    () =>
      timelines.map((timeline) => {
        const lastEvent = timeline.events[timeline.events.length - 1];
        const status = lastEvent ? deriveStatus(lastEvent.step) : { label: 'Idle', tone: 'idle' };
        const keyLabel = timeline.context.sessionId ?? timeline.context.tileId ?? timeline.label;
        return {
          key: timeline.key,
          label: keyLabel,
          status,
          events: timeline.events,
        };
      }),
    [timelines],
  );

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
        {summary.length === 0 ? (
          <p className="text-xs text-muted-foreground">Connection events will appear here as you interact with tiles.</p>
        ) : (
          <div className="space-y-3">
            {summary.map((item) => (
              <section key={item.key} className="rounded-lg border border-border bg-background/40 p-3">
                <div className="flex items-center justify-between text-xs text-muted-foreground">
                  <div className="font-mono text-[11px] text-foreground">{item.label}</div>
                  <span
                    className={cn(
                      'inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide',
                      item.status.tone === 'ok' && 'bg-emerald-100 text-emerald-900 dark:bg-emerald-900/30 dark:text-emerald-200',
                      item.status.tone === 'warn' && 'bg-amber-100 text-amber-900 dark:bg-amber-900/30 dark:text-amber-100',
                      item.status.tone === 'error' && 'bg-rose-100 text-rose-900 dark:bg-rose-900/30 dark:text-rose-100',
                      item.status.tone === 'idle' && 'bg-muted text-foreground',
                    )}
                  >
                    {item.status.label}
                  </span>
                </div>
                <ol className="mt-2 max-h-56 space-y-1 overflow-y-auto pr-1">
                  {item.events.map((event) => {
                    const detailText = describeDetail(event.detail);
                    return (
                      <li key={event.id} className="text-xs text-muted-foreground">
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
              </section>
            ))}
          </div>
        )}
      </div>
    </aside>
  );
}
