import { useEffect, useRef, useState } from 'react';
import { SessionSummary, eventsSseUrl, stateSseUrl } from '../lib/api';
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
  const stRef = useRef<EventSource | null>(null);
  const [events, setEvents] = useState<string[]>([]);

  useEffect(() => {
    evRef.current?.close();
    stRef.current?.close();
    setEvents([]);
    if (!open || !session) return;
    const ev = new EventSource(eventsSseUrl(session.session_id, managerUrl, token || undefined));
    ev.addEventListener('controller_event', (msg: MessageEvent) => setEvents((p) => [msg.data, ...p].slice(0, 200)));
    const st = new EventSource(stateSseUrl(session.session_id, managerUrl, token || undefined));
    st.addEventListener('state', (msg: MessageEvent) => setEvents((p) => [msg.data, ...p].slice(0, 200)));
    evRef.current = ev;
    stRef.current = st;
    return () => {
      ev.close();
      st.close();
    };
  }, [open, session, managerUrl, token]);

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <div className="flex h-full flex-col">
        <div className="border-b border-neutral-200 p-3">
          <div className="text-sm font-semibold">Session Detail</div>
          {session && <div className="font-mono text-xs text-neutral-600">{session.session_id}</div>}
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-3">
          {events.length === 0 ? (
            <div className="text-sm text-neutral-600">No events yet…</div>
          ) : (
            <div className="space-y-1">
              {events.map((e, i) => (
                <pre key={i} className="whitespace-pre-wrap break-words rounded border border-neutral-100 bg-neutral-50 p-2 text-[11px] text-neutral-800">{e}</pre>
              ))}
            </div>
          )}
        </div>
      </div>
    </Sheet>
  );
}

