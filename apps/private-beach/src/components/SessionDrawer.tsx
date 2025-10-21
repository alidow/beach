import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ControllerEvent, SessionSummary, fetchControllerEvents } from '../lib/api';
import { Sheet } from './ui/sheet';

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
          params.since_ms = latestTimestampRef.current;
          params.limit = 50;
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
    const timestamp = new Date(event.timestamp_ms).toLocaleString();
    const reason = event.reason ? event.reason : null;
    const controllerToken = event.controller_token ? redactToken(event.controller_token) : null;
    return (
      <div key={event.id} className="space-y-2 rounded border border-border bg-muted/40 p-2">
        <div className="flex items-center justify-between">
          <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">{event.event_type}</span>
          <span className="text-[11px] text-muted-foreground">{timestamp}</span>
        </div>
        {reason && <div className="text-xs text-muted-foreground">Reason: {reason}</div>}
        {controllerToken && (
          <div className="text-xs text-muted-foreground">Token: {controllerToken}</div>
        )}
        <pre className="whitespace-pre-wrap break-words text-[11px] text-muted-foreground">
          {JSON.stringify(event, null, 2)}
        </pre>
      </div>
    );
  };

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <div className="flex h-full flex-col">
        <div className="border-b border-border p-3">
          <div className="text-sm font-semibold">Session Detail</div>
          {session && <div className="font-mono text-xs text-muted-foreground">{session.session_id}</div>}
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-3">
          {!effectiveToken ? (
            <div className="text-sm text-muted-foreground">Sign in to view session events.</div>
          ) : events.length === 0 ? (
            <div className="text-sm text-muted-foreground">No events yet…</div>
          ) : (
            <div className="space-y-2">
              {events.map((event) => renderEvent(event))}
            </div>
          )}
        </div>
      </div>
    </Sheet>
  );
}
