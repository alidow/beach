import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ControllerEvent, SessionSummary, fetchControllerEvents } from '../lib/api';
import { Sheet } from './ui/sheet';
import { Badge } from './ui/badge';

type Props = {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  session: SessionSummary | null;
  managerUrl: string;
  token: string | null;
};

export default function SessionDrawer({ open, onOpenChange, session, managerUrl, token }: Props) {
  const pollRef = useRef<number | null>(null);
  const seenIdsRef = useRef<Set<string>>(new Set());
  const latestTimestampRef = useRef<number>(0);
  const [events, setEvents] = useState<ControllerEvent[]>([]);
  const effectiveToken = useMemo(() => (token && token.trim().length > 0 ? token.trim() : null), [token]);
  const redactToken = (value: string | null) => {
    if (!value) return '(none)';
    if (value.length <= 8) return value;
    return `${value.slice(0, 4)}…${value.slice(-4)}`;
  };

  const formatEventType = useCallback((value: string) => {
    if (!value) return 'Event';
    return value
      .split('_')
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(' ');
  }, []);

  const formatRelative = useCallback((timestampMs: number) => {
    const diff = Date.now() - timestampMs;
    if (diff <= 0) return 'just now';
    const seconds = Math.floor(diff / 1000);
    if (seconds < 60) return `${seconds}s ago`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `${hours}h ago`;
    const days = Math.floor(hours / 24);
    if (days < 7) return `${days}d ago`;
    const weeks = Math.floor(days / 7);
    if (weeks < 5) return `${weeks}w ago`;
    const months = Math.floor(days / 30);
    if (months < 12) return `${months}mo ago`;
    const years = Math.floor(days / 365);
    return `${years}y ago`;
  }, []);
  const resetState = useCallback(() => {
    if (pollRef.current !== null) {
      window.clearInterval(pollRef.current);
      pollRef.current = null;
    }
    seenIdsRef.current = new Set();
    latestTimestampRef.current = 0;
    setEvents([]);
  }, []);

  useEffect(() => {
    resetState();
    if (!open || !session || !effectiveToken) {
      return;
    }

    let cancelled = false;

    const ingest = (incoming: ControllerEvent[]) => {
      if (incoming.length === 0) {
        return;
      }
      const seen = seenIdsRef.current;
      const fresh: ControllerEvent[] = [];
      for (const event of incoming) {
        if (seen.has(event.id)) {
          continue;
        }
        seen.add(event.id);
        latestTimestampRef.current = Math.max(latestTimestampRef.current, event.timestamp_ms);
        fresh.push(event);
      }
      if (fresh.length > 0) {
        setEvents((prev) => {
          const combined = [...fresh, ...prev];
          if (combined.length <= 200) {
            return combined;
          }
          return combined.slice(0, 200);
        });
      }
    };

    const fetchBatch = async (incremental: boolean) => {
      try {
        const params: { since_ms?: number; limit?: number } = {};
        if (incremental && latestTimestampRef.current > 0) {
          params.since_ms = latestTimestampRef.current + 1;
          params.limit = 100;
        } else {
          params.limit = 200;
        }
        const result = await fetchControllerEvents(session.session_id, effectiveToken, managerUrl, params);
        if (!cancelled) {
          ingest(result);
        }
      } catch (error) {
        if (!cancelled) {
          console.error('[drawer] controller events fetch failed', {
            sessionId: session.session_id,
            managerUrl,
            token: redactToken(effectiveToken),
            error,
          });
        }
      }
    };

    fetchBatch(false);
    pollRef.current = window.setInterval(() => {
      fetchBatch(true);
    }, 5000);

    return () => {
      cancelled = true;
      resetState();
    };
  }, [open, session, managerUrl, effectiveToken, resetState]);

  const renderEvent = (event: ControllerEvent) => {
    const absolute = new Date(event.timestamp_ms).toLocaleString();
    const relative = formatRelative(event.timestamp_ms);
    const reason = event.reason ? event.reason : null;
    const controllerToken = event.controller_token ? redactToken(event.controller_token) : null;
    const controllerAccount = event.controller_account_id ? `acct_${event.controller_account_id.slice(0, 8)}` : null;
    const issuedBy = event.issued_by_account_id ? `acct_${event.issued_by_account_id.slice(0, 8)}` : null;
    const typeLabel = formatEventType(event.event_type);

    return (
      <div
        key={event.id}
        className="space-y-3 rounded-lg border border-border/60 bg-background/70 p-3 shadow-sm backdrop-blur-sm"
      >
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant="muted" className="uppercase tracking-[0.2em] text-[10px]">
              {typeLabel}
            </Badge>
            {controllerToken && (
              <span className="rounded bg-muted/40 px-2 py-0.5 text-[11px] font-mono text-muted-foreground">
                Token {controllerToken}
              </span>
            )}
            {controllerAccount && (
              <span className="rounded bg-muted/30 px-2 py-0.5 text-[11px] text-muted-foreground">
                Controller {controllerAccount}
              </span>
            )}
          </div>
          <div className="text-[11px] text-muted-foreground">
            {absolute} · {relative}
          </div>
        </div>
        <dl className="grid gap-1 text-xs text-muted-foreground">
          {reason && (
            <div>
              <span className="font-medium text-foreground">Reason:</span> {reason}
            </div>
          )}
          {issuedBy && (
            <div>
              <span className="font-medium text-foreground">Issued by:</span> {issuedBy}
            </div>
          )}
        </dl>
        <details className="rounded border border-border/40 bg-muted/15 px-3 py-2 text-[11px] text-muted-foreground">
          <summary className="cursor-pointer font-medium text-muted-foreground/90">Raw event</summary>
          <pre className="mt-2 whitespace-pre-wrap break-words">
            {JSON.stringify(event, null, 2)}
          </pre>
        </details>
      </div>
    );
  };

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <div className="flex h-full flex-col">
        <div className="border-b border-border px-4 py-3">
          <div className="flex flex-col gap-2">
            <div className="text-sm font-semibold">Session Detail</div>
            {session && (
              <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                <span className="rounded border border-border/60 bg-background/80 px-2 py-1 font-mono text-[11px] text-foreground">
                  {session.session_id}
                </span>
                <Badge variant="muted" className="uppercase tracking-[0.18em]">
                  {session.harness_type}
                </Badge>
                <Badge
                  variant={session.last_health?.degraded ? 'warning' : 'success'}
                  className="uppercase tracking-[0.18em] text-[10px]"
                >
                  {session.last_health?.degraded ? 'Degraded' : 'Healthy'}
                </Badge>
                <span className="rounded bg-muted/30 px-2 py-0.5 text-[11px] text-muted-foreground">
                  Pending {session.pending_actions}/{session.pending_unacked}
                </span>
                <span className="rounded bg-muted/30 px-2 py-0.5 text-[11px] text-muted-foreground">
                  Location {session.location_hint ?? '—'}
                </span>
              </div>
            )}
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-auto px-4 py-3">
          {!effectiveToken ? (
            <div className="rounded-lg border border-dashed border-border/70 bg-muted/20 p-4 text-sm text-muted-foreground">
              Sign in to view session events.
            </div>
          ) : events.length === 0 ? (
            <div className="rounded-lg border border-dashed border-border/70 bg-muted/20 p-4 text-sm text-muted-foreground">
              No controller events yet.
            </div>
          ) : (
            <div className="space-y-3">
              {events.map((event) => renderEvent(event))}
            </div>
          )}
        </div>
      </div>
    </Sheet>
  );
}
