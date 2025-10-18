import Link from 'next/link';
import { useEffect, useMemo, useState } from 'react';
import { PrivateBeach, listBeaches } from '../lib/beaches';
import { Select } from './ui/select';
import { Button } from './ui/button';

type Props = {
  current?: PrivateBeach | null;
  onSwitch?: (id: string) => void;
  right?: React.ReactNode;
};

export default function TopNav({ current, onSwitch, right }: Props) {
  const [beaches, setBeaches] = useState<PrivateBeach[]>([]);
  useEffect(() => {
    // Load client-only data after mount to avoid SSR hydration mismatches
    setBeaches(listBeaches());
  }, []);

  const value = useMemo(() => {
    if (!current) return '';
    const ids = new Set(beaches.map((b) => b.id));
    return ids.has(current.id) ? current.id : '';
  }, [current, beaches]);
  return (
    <div className="sticky top-0 z-40 flex h-12 items-center justify-between border-b border-neutral-200 bg-white/90 px-3 backdrop-blur">
      <div className="flex items-center gap-3">
        <Link href="/beaches" className="text-sm font-semibold">Private Beach</Link>
        <div className="flex items-center gap-2">
          <span className="text-xs text-neutral-600">Beach</span>
          <Select
            value={value}
            onChange={(v) => onSwitch && onSwitch(v)}
            options={[{ value: '', label: '—' }, ...beaches.map((b) => ({ value: b.id, label: b.name }))]}
          />
          <Link href="/beaches/new"><Button variant="outline" size="sm">New</Button></Link>
        </div>
      </div>
      <div className="flex items-center gap-2">{right}</div>
    </div>
  );
}
