'use client';

import { useState, useTransition } from 'react';
import { useRouter } from 'next/navigation';
import { createBeachAction } from '@/app/beaches/actions';
import { Button } from '../../../private-beach/src/components/ui/button';
import { Input } from '../../../private-beach/src/components/ui/input';

export function CreateBeachButton() {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('My Private Beach');
  const [slug, setSlug] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [isPending, startTransition] = useTransition();
  const router = useRouter();

  const debug = (...args: unknown[]) => {
    if (process.env.NODE_ENV !== 'production') {
      // eslint-disable-next-line no-console
      console.log('[CreateBeachButton]', ...args);
    }
  };

  const reset = () => {
    setName('My Private Beach');
    setSlug('');
    setError(null);
  };

  const handleOpenChange = (next: boolean, reason = 'unspecified') => {
    debug('handleOpenChange', { next, reason });
    if (!next) {
      reset();
    }
    setOpen(next);
  };

  const handleCreate = () => {
    debug('handleCreate invoked', { name, slug });
    setError(null);
    startTransition(async () => {
      debug('createBeachAction dispatched', {
        nameLength: name.length,
        slugLength: slug.length,
        slugProvided: Boolean(slug.trim()),
      });
      try {
        const result = await createBeachAction({ name, slug });
        if (result.success) {
          debug('createBeachAction success', { id: result.id });
          handleOpenChange(false, 'create-success');
          router.push(`/beaches/${result.id}`);
        } else {
          debug('createBeachAction failure', { error: result.error });
          setError(result.error);
        }
      } catch (actionError) {
        const message = actionError instanceof Error ? actionError.message : String(actionError);
        debug('createBeachAction threw', { message });
        setError(message);
      }
    });
  };

  debug('render', { open, nameLength: name.length, slugLength: slug.length, hasError: Boolean(error), isPending });

  return (
    <>
      <Button size="sm" onClick={() => handleOpenChange(true, 'trigger-click')}>
        New Beach
      </Button>
      {open ? (
        <div
          className="fixed inset-0 z-50"
          role="dialog"
          aria-modal="true"
          aria-labelledby="create-beach-dialog-title"
          aria-describedby="create-beach-dialog-description"
        >
          <div
            className="absolute inset-0 bg-black/50 backdrop-blur-sm transition-opacity dark:bg-black/70"
            onClick={() => handleOpenChange(false, 'backdrop-click')}
          />
          <div className="absolute left-1/2 top-1/2 w-[420px] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-border bg-card text-card-foreground shadow-xl">
            <div className="p-4">
              <h3 id="create-beach-dialog-title" className="mb-1 text-sm font-semibold">
                Create Private Beach
              </h3>
              <p id="create-beach-dialog-description" className="mb-3 text-sm text-muted-foreground">
                Provide a name and optional slug. We will create an empty workspace for you.
              </p>
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
            </div>
            <div className="border-t border-border p-3">
              <div className="flex justify-end gap-2">
                <Button variant="ghost" onClick={() => handleOpenChange(false, 'cancel-click')} disabled={isPending}>
                  Cancel
                </Button>
                <Button onClick={handleCreate} disabled={isPending}>
                  {isPending ? 'Creating…' : 'Create'}
                </Button>
              </div>
            </div>
          </div>
        </div>
      ) : null}
    </>
  );
}
