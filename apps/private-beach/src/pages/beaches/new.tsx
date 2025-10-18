import { useRouter } from 'next/router';
import { useEffect, useState } from 'react';
import TopNav from '../../components/TopNav';
import { Button } from '../../components/ui/button';
import { Card, CardContent, CardHeader } from '../../components/ui/card';
import { Input } from '../../components/ui/input';
import { PrivateBeach, ensureId, upsertBeach } from '../../lib/beaches';

export default function NewBeach() {
  const router = useRouter();
  const [name, setName] = useState('My Private Beach');
  const [id, setId] = useState('');
  const [managerUrl, setManagerUrl] = useState<string>('');
  const [token, setToken] = useState<string>('test-token');

  useEffect(() => {
    setManagerUrl(process.env.NEXT_PUBLIC_MANAGER_URL || 'http://localhost:8080');
  }, []);

  function onGenerate() {
    setId(ensureId());
  }

  function onCreate() {
    const beach: PrivateBeach = {
      id: ensureId(id),
      name: name.trim() || 'Private Beach',
      managerUrl: managerUrl.trim() || 'http://localhost:8080',
      token: token || null,
      createdAt: Date.now(),
    };
    upsertBeach(beach);
    router.push(`/beaches/${beach.id}`);
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
                <div className="mb-1 flex items-center justify-between">
                  <label className="text-xs text-neutral-700">ID (UUID)</label>
                  <button className="text-xs text-neutral-600 underline" onClick={onGenerate}>Generate</button>
                </div>
                <Input value={id} onChange={(e) => setId(e.target.value)} placeholder="leave blank to auto-generate" />
              </div>
              <div>
                <label className="mb-1 block text-xs text-neutral-700">Manager URL</label>
                <Input value={managerUrl} onChange={(e) => setManagerUrl(e.target.value)} placeholder="http://localhost:8080" />
              </div>
              <div>
                <label className="mb-1 block text-xs text-neutral-700">Token (dev)</label>
                <Input value={token} onChange={(e) => setToken(e.target.value)} placeholder="test-token" />
              </div>
              <div className="pt-2">
                <Button onClick={onCreate}>Create</Button>
              </div>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

