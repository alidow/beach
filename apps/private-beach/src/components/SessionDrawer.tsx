import { useEffect, useMemo, useRef, useState } from 'react';
import { SessionSummary, eventsSseUrl } from '../lib/api';
import { Sheet } from './ui/sheet';

type Props = {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  session: SessionSummary | null;
  managerUrl: string;
  token: string | null;
};

export default function SessionDrawer({ open, onOpenChange, session, managerUrl, token }: Props) {
  const evRef = useRef<EventSource | null>(null);
  const [events, setEvents] = useState<string[]>([]);
  const effectiveToken = useMemo(() => (token && token.trim().length > 0 ? token.trim() : null), [token]);
  const redactToken = (value: string | null) => {
    if (!value) return '(none)';
    if (value.length <= 8) return value;
    return `${value.slice(0, 4)}…${value.slice(-4)}`;
  };

  useEffect(() => {
    evRef.current?.close();
    setEvents([]);
    if (!open || !session || !effectiveToken) return;
    const eventsUrl = eventsSseUrl(session.session_id, managerUrl, effectiveToken);
    console.info('[drawer] opening SSE streams', {
      sessionId: session.session_id,
      managerUrl,
      eventsUrl,
      token: redactToken(effectiveToken),
    });
    const ev = new EventSource(eventsUrl);
    ev.addEventListener('controller_event', (msg: MessageEvent) => setEvents((p) => [msg.data, ...p].slice(0, 200)));
    ev.onerror = (err) => {
      console.error('[drawer] controller_event stream error', {
        sessionId: session.session_id,
        managerUrl,
        eventsUrl,
        token: redactToken(effectiveToken),
        error: err,
      });
    };
    evRef.current = ev;
    return () => {
      console.debug('[drawer] closing SSE streams', {
        sessionId: session.session_id,
        managerUrl,
      });
      ev.close();
    };
  }, [open, session, managerUrl, effectiveToken]);

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
            <div className="space-y-1">
              {events.map((e, i) => (
                <pre key={i} className="whitespace-pre-wrap break-words rounded border border-border bg-muted p-2 text-[11px] text-muted-foreground">{e}</pre>
              ))}
            </div>
          )}
        </div>
      </div>
    </Sheet>
  );
}
