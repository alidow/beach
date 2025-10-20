import { useRouter } from 'next/router';
import { useEffect, useMemo, useState } from 'react';
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

  useEffect(() => {
    if (!id) return;
    getBeachMeta(id, null)
      .then((meta) => { setBeachId(meta.id); setName(meta.name); })
      .catch(() => {});
  }, [id]);

  function onSave() {
    if (!beachId) return;
    setSaving(true);
    updateBeach(beachId, { name: name.trim() || name }, null)
      .then(() => alert('Saved'))
      .finally(() => setSaving(false));
  }

  return (
    <div className="min-h-screen">
      <TopNav currentId={id} onSwitch={(v) => router.push(`/beaches/${v}`)} />
      <div className="mx-auto max-w-xl p-4">
        {!id ? (
          <div className="text-sm text-neutral-600">Loading…</div>
        ) : (
          <Card>
            <CardHeader>
              <div>
                <div className="text-sm font-semibold">Settings</div>
                <div className="text-xs text-neutral-600">{id}</div>
              </div>
            </CardHeader>
            <CardContent>
              <div className="space-y-3">
                <div>
                  <label className="mb-1 block text-xs text-neutral-700">Name</label>
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
