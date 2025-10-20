import { useRouter } from 'next/router';
import { useEffect, useState } from 'react';
import TopNav from '../../components/TopNav';
import { Button } from '../../components/ui/button';
import { Card, CardContent, CardHeader } from '../../components/ui/card';
import { Input } from '../../components/ui/input';
import { createBeach } from '../../lib/api';

export default function NewBeach() {
  const router = useRouter();
  const [name, setName] = useState('My Private Beach');
  const [slug, setSlug] = useState('');
  const [creating, setCreating] = useState(false);

  async function onCreate() {
    setCreating(true);
    try {
      const created = await createBeach(name.trim() || 'Private Beach', slug.trim() || undefined, null);
      router.push(`/beaches/${created.id}`);
    } finally {
      setCreating(false);
    }
  }

  return (
    <div className="min-h-screen">
      <TopNav />
      <div className="mx-auto max-w-xl p-4">
        <Card>
          <CardHeader>
            <h1 className="text-base font-semibold">Create Private Beach</h1>
          </CardHeader>
          <CardContent>
            <div className="space-y-3">
              <div>
                <label className="mb-1 block text-xs text-neutral-700">Name</label>
                <Input value={name} onChange={(e) => setName(e.target.value)} />
              </div>
              <div>
                <label className="mb-1 block text-xs text-neutral-700">Slug (optional)</label>
                <Input value={slug} onChange={(e) => setSlug(e.target.value)} placeholder="lowercase-with-dashes" />
              </div>
              <div className="pt-2">
                <Button onClick={onCreate} disabled={creating}>{creating ? 'Creating…' : 'Create'}</Button>
              </div>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
