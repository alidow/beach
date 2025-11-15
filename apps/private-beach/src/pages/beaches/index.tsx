import Link from 'next/link';
import { useEffect, useState } from 'react';
import { useAuth } from '@clerk/nextjs';
import TopNav from '../../components/TopNav';
import { Card, CardContent, CardHeader } from '../../components/ui/card';
import { Button } from '../../components/ui/button';
import { Input } from '../../components/ui/input';
import { BeachSummary, listBeaches } from '../../lib/api';
import { useManagerToken } from '../../hooks/useManagerToken';

export default function BeachesIndex() {
  const [beaches, setBeaches] = useState<BeachSummary[]>([]);
  const [query, setQuery] = useState('');
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

  const filtered = beaches.filter((b) => b.name.toLowerCase().includes(query.toLowerCase()) || b.id.startsWith(query));

  function onDelete(_id: string) {}

  return (
    <div className="min-h-screen">
      <TopNav />
      <div className="mx-auto max-w-4xl p-4">
        <div className="mb-4 flex items-center justify-between">
          <h1 className="text-lg font-semibold">Your Private Beaches</h1>
          <Link href="/beaches/new"><Button>New Beach</Button></Link>
        </div>
        <div className="mb-3"><Input placeholder="Search by name or id…" value={query} onChange={(e) => setQuery(e.target.value)} /></div>
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
          {filtered.length === 0 ? (
            <div className="text-sm text-muted-foreground">No beaches yet. Create one to get started.</div>
          ) : (
            filtered.map((b) => (
              <Card key={b.id}>
                <CardHeader>
                  <div className="flex items-center justify-between">
                    <div>
                      <div className="text-sm font-semibold">{b.name}</div>
                      <div className="text-xs text-muted-foreground">{b.id}</div>
                    </div>
                    <div className="flex items-center gap-2">
                      <Link href={`/beaches/${b.id}`}><Button size="sm">Open</Button></Link>
                      <Link href={`/beaches/${b.id}/settings`}><Button variant="outline" size="sm">Settings</Button></Link>
                      <Button variant="ghost" size="sm" onClick={() => onDelete(b.id)}>Remove</Button>
                    </div>
                  </div>
                </CardHeader>
                {/* no extra content for now */}
              </Card>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
