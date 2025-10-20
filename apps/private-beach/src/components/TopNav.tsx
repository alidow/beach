import Link from 'next/link';
import { useEffect, useMemo, useState } from 'react';
import { BeachSummary, listBeaches } from '../lib/api';
import { Select } from './ui/select';
import { Button } from './ui/button';

type Props = { currentId?: string; onSwitch?: (id: string) => void; right?: React.ReactNode };

export default function TopNav({ currentId, onSwitch, right }: Props) {
  const [beaches, setBeaches] = useState<BeachSummary[]>([]);
  useEffect(() => {
    listBeaches(null).then(setBeaches).catch(() => setBeaches([]));
  }, []);

  const value = useMemo(() => currentId || '', [currentId]);
  return (
    <div className="sticky top-0 z-40 flex h-12 items-center justify-between border-b border-neutral-200 bg-white/90 px-3 backdrop-blur">
      <div className="flex items-center gap-3">
        <Link href="/beaches" className="text-sm font-semibold">Private Beach</Link>
        <div className="flex items-center gap-2">
          <span className="text-xs text-neutral-600">Beach</span>
          <Select value={value} onChange={(v) => onSwitch && onSwitch(v)} options={[{ value: '', label: '—' }, ...beaches.map((b) => ({ value: b.id, label: b.name }))]} />
          <Link href="/beaches/new"><Button variant="outline" size="sm">New</Button></Link>
        </div>
      </div>
      <div className="flex items-center gap-2">{right}</div>
    </div>
  );
}
