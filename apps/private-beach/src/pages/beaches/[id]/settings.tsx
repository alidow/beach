import { useRouter } from 'next/router';
import { useEffect, useState } from 'react';
import { useAuth } from '@clerk/nextjs';
import TopNav from '../../../components/TopNav';
import { Button } from '../../../components/ui/button';
import { Card, CardContent, CardHeader } from '../../../components/ui/card';
import { Input } from '../../../components/ui/input';
import { getBeachMeta, updateBeach } from '../../../lib/api';

export default function BeachSettings() {
  const router = useRouter();
  const { id } = router.query as { id?: string };
  const [beachId, setBeachId] = useState<string>('');
  const [name, setName] = useState('');
  const [saving, setSaving] = useState(false);
  const { isLoaded, isSignedIn, getToken } = useAuth();
  const tokenTemplate = process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE;

  useEffect(() => {
    if (!id || !isLoaded || !isSignedIn) return;
    let active = true;
    (async () => {
      try {
        const token = await getToken(
          tokenTemplate ? { template: tokenTemplate } : undefined,
        );
        if (!token || !active) return;
        const meta = await getBeachMeta(id, token);
        if (!active) return;
        setBeachId(meta.id);
        setName(meta.name);
      } catch {
        if (active) {
          setBeachId('');
          setName('');
        }
      }
    })();
    return () => {
      active = false;
    };
  }, [id, isLoaded, isSignedIn, getToken, tokenTemplate]);

  function onSave() {
    if (!beachId) return;
    if (!isLoaded || !isSignedIn) return;
    setSaving(true);
    (async () => {
      try {
        const token = await getToken(
          tokenTemplate ? { template: tokenTemplate } : undefined,
        );
        if (!token) throw new Error('Missing manager auth token');
        await updateBeach(beachId, { name: name.trim() || name }, token);
        alert('Saved');
      } catch (err) {
        console.error(err);
      } finally {
        setSaving(false);
      }
    })();
  }

  return (
    <div className="min-h-screen">
      <TopNav currentId={id} onSwitch={(v) => router.push(`/beaches/${v}`)} />
      <div className="mx-auto max-w-xl p-4">
        {!id ? (
          <div className="text-sm text-muted-foreground">Loading…</div>
        ) : (
          <Card>
            <CardHeader>
              <div>
                <div className="text-sm font-semibold">Settings</div>
                <div className="text-xs text-muted-foreground">{id}</div>
              </div>
            </CardHeader>
            <CardContent>
              <div className="space-y-3">
                <div>
                  <label className="mb-1 block text-xs text-muted-foreground">Name</label>
                  <Input value={name} onChange={(e) => setName(e.target.value)} />
                </div>
                <div className="pt-2">
                  <Button onClick={onSave} disabled={saving}>{saving ? 'Saving…' : 'Save'}</Button>
                </div>
              </div>
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  );
}
