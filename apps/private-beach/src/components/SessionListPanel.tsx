import { useMemo, useState } from 'react';
import { SessionSummary } from '../lib/api';
import { Input } from './ui/input';
import { Badge } from './ui/badge';
import { Button } from './ui/button';

type Props = {
  sessions: SessionSummary[];
  onAdd: (id: string) => void;
};

export default function SessionListPanel({ sessions, onAdd }: Props) {
  const [query, setQuery] = useState('');
  const [typeFilter, setTypeFilter] = useState<string>('all');

  const filtered = useMemo(() => {
    return sessions.filter((s) => {
      const q = query.toLowerCase();
      const matchesQ = !q || s.session_id.includes(q) || (s.location_hint || '').toLowerCase().includes(q) || s.harness_type.toLowerCase().includes(q);
      const matchesType = typeFilter === 'all' || s.harness_type === typeFilter;
      return matchesQ && matchesType;
    });
  }, [sessions, query, typeFilter]);

  const harnessTypes = Array.from(new Set(sessions.map((s) => s.harness_type)));

  return (
    <div className="flex h-full flex-col">
      <div className="border-b border-neutral-200 p-2">
        <Input placeholder="Search sessions…" value={query} onChange={(e) => setQuery(e.target.value)} />
        <div className="mt-2 flex items-center gap-2">
          <button className={`text-xs ${typeFilter === 'all' ? 'font-semibold' : 'text-neutral-600'}`} onClick={() => setTypeFilter('all')}>All</button>
          {harnessTypes.map((t) => (
            <button key={t} className={`text-xs ${typeFilter === t ? 'font-semibold' : 'text-neutral-600'}`} onClick={() => setTypeFilter(t)}>{t}</button>
          ))}
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-2">
        {filtered.length === 0 ? (
          <div className="p-2 text-sm text-neutral-600">No sessions</div>
        ) : (
          <ul className="space-y-2">
            {filtered.map((s) => (
              <li key={s.session_id} className="rounded-md border border-neutral-200 bg-white p-2">
                <div className="flex items-center justify-between">
                  <div>
                    <div className="font-mono text-xs">{s.session_id.slice(0, 8)}</div>
                    <div className="flex items-center gap-2">
                      <Badge variant="muted">{s.harness_type}</Badge>
                      <span className="text-[11px] text-neutral-600">{s.location_hint || '—'}</span>
                    </div>
                  </div>
                  <div className="flex items-center gap-2">
                    <span className="text-[11px] text-neutral-600">{s.pending_actions}/{s.pending_unacked}</span>
                    <Button size="sm" variant="outline" onClick={() => onAdd(s.session_id)}>Add</Button>
                  </div>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

