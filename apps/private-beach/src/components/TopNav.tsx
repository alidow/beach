import Link from 'next/link';
import { useEffect, useMemo, useState } from 'react';
import { SignedIn, SignedOut, SignInButton, UserButton, useAuth } from '@clerk/nextjs';
import { BeachSummary, listBeaches } from '../lib/api';
import { Select } from './ui/select';
import { Button } from './ui/button';
import { useManagerToken } from '../hooks/useManagerToken';

type Props = { currentId?: string; onSwitch?: (id: string) => void; right?: React.ReactNode };

export default function TopNav({ currentId, onSwitch, right }: Props) {
  const [beaches, setBeaches] = useState<BeachSummary[]>([]);
  const { isLoaded, isSignedIn } = useAuth();
  const { token: managerToken, refresh: refreshManagerToken } = useManagerToken(isLoaded && isSignedIn);

  useEffect(() => {
    if (!isLoaded || !isSignedIn) {
      setBeaches([]);
      return;
    }
    if (!managerToken || managerToken.trim().length === 0) {
      setBeaches([]);
      return;
    }
    let active = true;
    (async () => {
      try {
        const data = await listBeaches(managerToken);
        if (active) setBeaches(data);
      } catch {
        if (active) {
          setBeaches([]);
          await refreshManagerToken().catch(() => {});
        }
      }
    })();
    return () => {
      active = false;
    };
  }, [isLoaded, isSignedIn, managerToken, refreshManagerToken]);

  const value = useMemo(() => currentId || '', [currentId]);
  return (
    <div className="sticky top-0 z-40 flex h-12 items-center justify-between border-b border-border bg-background/80 px-3 backdrop-blur supports-[backdrop-filter]:bg-background/60">
      <div className="flex items-center gap-3">
        <Link href="/beaches" className="text-sm font-semibold">Private Beach</Link>
        <div className="flex items-center gap-2">
          <span className="text-xs text-muted-foreground">Beach</span>
          <Select value={value} onChange={(v) => onSwitch && onSwitch(v)} options={[{ value: '', label: '—' }, ...beaches.map((b) => ({ value: b.id, label: b.name }))]} />
          <Link href="/beaches/new"><Button variant="outline" size="sm">New</Button></Link>
        </div>
      </div>
      <div className="flex items-center gap-2">
        {right}
        <SignedIn>
          <UserButton afterSignOutUrl="/sign-in" />
        </SignedIn>
        <SignedOut>
          <SignInButton />
        </SignedOut>
      </div>
    </div>
  );
}
