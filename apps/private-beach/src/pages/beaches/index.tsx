import Link from 'next/link';
import { useEffect, useState } from 'react';
import TopNav from '../../components/TopNav';
import { Card, CardContent, CardHeader } from '../../components/ui/card';
import { Button } from '../../components/ui/button';
import { Input } from '../../components/ui/input';
import { PrivateBeach, deleteBeach, listBeaches } from '../../lib/beaches';

export default function BeachesIndex() {
  const [beaches, setBeaches] = useState<PrivateBeach[]>([]);
  const [query, setQuery] = useState('');
  useEffect(() => setBeaches(listBeaches()), []);

  const filtered = beaches.filter((b) => b.name.toLowerCase().includes(query.toLowerCase()) || b.id.startsWith(query));

  function onDelete(id: string) {
    if (!confirm('Delete this beach from local list? (Does not delete server data)')) return;
    deleteBeach(id);
    setBeaches(listBeaches());
  }

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
            <div className="text-sm text-neutral-600">No beaches yet. Create one to get started.</div>
          ) : (
            filtered.map((b) => (
              <Card key={b.id}>
                <CardHeader>
                  <div className="flex items-center justify-between">
                    <div>
                      <div className="text-sm font-semibold">{b.name}</div>
                      <div className="text-xs text-neutral-500">{b.id}</div>
                    </div>
                    <div className="flex items-center gap-2">
                      <Link href={`/beaches/${b.id}`}><Button size="sm">Open</Button></Link>
                      <Link href={`/beaches/${b.id}/settings`}><Button variant="outline" size="sm">Settings</Button></Link>
                      <Button variant="ghost" size="sm" onClick={() => onDelete(b.id)}>Remove</Button>
                    </div>
                  </div>
                </CardHeader>
                <CardContent>
                  <div className="text-xs text-neutral-600">Manager: {b.managerUrl}</div>
                </CardContent>
              </Card>
            ))
          )}
        </div>
      </div>
    </div>
  );
}

