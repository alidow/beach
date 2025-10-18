import { useRouter } from 'next/router';
import { useEffect, useMemo, useState } from 'react';
import TopNav from '../../../components/TopNav';
import { Button } from '../../../components/ui/button';
import { Card, CardContent, CardHeader } from '../../../components/ui/card';
import { Input } from '../../../components/ui/input';
import { PrivateBeach, getBeach, upsertBeach } from '../../../lib/beaches';

export default function BeachSettings() {
  const router = useRouter();
  const { id } = router.query as { id?: string };
  const [beach, setBeach] = useState<PrivateBeach | null>(null);
  const [name, setName] = useState('');
  const [managerUrl, setManagerUrl] = useState('');
  const [token, setToken] = useState<string>('');

  useEffect(() => {
    if (!id) return;
    const b = getBeach(id);
    if (b) {
      setBeach(b);
      setName(b.name);
      setManagerUrl(b.managerUrl);
      setToken(b.token || '');
    }
  }, [id]);

  function onSave() {
    if (!beach) return;
    upsertBeach({ id: beach.id, name: name.trim() || beach.name, managerUrl: managerUrl.trim(), token: token || null, createdAt: beach.createdAt });
    alert('Saved');
  }

  return (
    <div className="min-h-screen">
      <TopNav current={beach} onSwitch={(v) => router.push(`/beaches/${v}`)} />
      <div className="mx-auto max-w-xl p-4">
        {!beach ? (
          <div className="text-sm text-neutral-600">Loading…</div>
        ) : (
          <Card>
            <CardHeader>
              <div>
                <div className="text-sm font-semibold">Settings</div>
                <div className="text-xs text-neutral-600">{beach.id}</div>
              </div>
            </CardHeader>
            <CardContent>
              <div className="space-y-3">
                <div>
                  <label className="mb-1 block text-xs text-neutral-700">Name</label>
                  <Input value={name} onChange={(e) => setName(e.target.value)} />
                </div>
                <div>
                  <label className="mb-1 block text-xs text-neutral-700">Manager URL</label>
                  <Input value={managerUrl} onChange={(e) => setManagerUrl(e.target.value)} />
                </div>
                <div>
                  <label className="mb-1 block text-xs text-neutral-700">Token (dev)</label>
                  <Input value={token} onChange={(e) => setToken(e.target.value)} />
                </div>
                <div className="pt-2">
                  <Button onClick={onSave}>Save</Button>
                </div>
              </div>
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  );
}

