'use client';

import { useRouter } from 'next/navigation';
import { useState, useTransition } from 'react';
import { Suspense } from 'react';
import { CreateBeachButton } from './CreateBeachButton';
import { Button } from '../../../private-beach/src/components/ui/button';

export function BeachesActions() {
  const router = useRouter();
  const [isPending, startTransition] = useTransition();
  const [error, setError] = useState<string | null>(null);

  const handleForceCreate = () => {
    setError(null);
    startTransition(async () => {
      try {
        const resp = await fetch('/api/test/create-beach', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
        });
        if (!resp.ok) {
          const detail = await resp.text().catch(() => resp.statusText);
          throw new Error(`status ${resp.status}: ${detail}`);
        }
        const data = await resp.json();
        const id = data.id as string;
        if (!id) throw new Error('missing id');
        router.push(`/beaches/${id}`);
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        setError(msg);
      }
    });
  };

  return (
    <div className="flex items-center gap-2">
      <Button size="sm" variant="secondary" onClick={handleForceCreate} disabled={isPending} data-testid="force-create-beach">
        {isPending ? 'Creating…' : 'New Beach'}
      </Button>
      {error ? <span className="text-xs text-destructive">{error}</span> : null}
      <Suspense fallback={null}>
        <CreateBeachButton />
      </Suspense>
    </div>
  );
}
