'use client';

import { useState, useTransition } from 'react';
import { useRouter } from 'next/navigation';
import { createBeachAction } from '@/app/beaches/actions';
import { Button } from '../../../private-beach/src/components/ui/button';
import { Dialog } from '../../../private-beach/src/components/ui/dialog';
import { Input } from '../../../private-beach/src/components/ui/input';

export function CreateBeachButton() {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('My Private Beach');
  const [slug, setSlug] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [isPending, startTransition] = useTransition();
  const router = useRouter();

  const reset = () => {
    setName('My Private Beach');
    setSlug('');
    setError(null);
  };

  const handleOpenChange = (next: boolean) => {
    if (!next) {
      reset();
    }
    setOpen(next);
  };

  const handleCreate = () => {
    setError(null);
    startTransition(async () => {
      const result = await createBeachAction({ name, slug });
      if (result.success) {
        setOpen(false);
        reset();
        router.push(`/beaches/${result.id}`);
      } else {
        setError(result.error);
      }
    });
  };

  return (
    <>
      <Button size="sm" onClick={() => handleOpenChange(true)}>
        New beach
      </Button>
      <Dialog
        open={open}
        onOpenChange={handleOpenChange}
        title="Create Private Beach"
        description="Provide a name and optional slug. We will create an empty workspace for you."
        footer={
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => handleOpenChange(false)} disabled={isPending}>
              Cancel
            </Button>
            <Button onClick={handleCreate} disabled={isPending}>
              {isPending ? 'Creating…' : 'Create'}
            </Button>
          </div>
        }
      >
        <div className="space-y-4 py-2">
          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground" htmlFor="beach-name">
              Name
            </label>
            <Input
              id="beach-name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="My Private Beach"
              autoFocus
            />
          </div>
          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground" htmlFor="beach-slug">
              Slug (optional)
            </label>
            <Input
              id="beach-slug"
              value={slug}
              onChange={(event) => setSlug(event.target.value)}
              placeholder="lowercase-with-dashes"
            />
          </div>
          {error ? <p className="text-xs text-destructive-foreground/90">{error}</p> : null}
        </div>
      </Dialog>
    </>
  );
}
