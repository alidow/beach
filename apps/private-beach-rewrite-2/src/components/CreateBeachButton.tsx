'use client';

import { useEffect, useMemo, useState, useTransition } from 'react';
import { createPortal } from 'react-dom';
import { useRouter } from 'next/navigation';
import { createBeachAction } from '@/app/beaches/actions';
import { Button } from '../../../private-beach/src/components/ui/button';
import { Input } from '../../../private-beach/src/components/ui/input';

export function CreateBeachButton() {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('My Private Beach');
  const [slug, setSlug] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [hasProvidedToken, setHasProvidedToken] = useState(false);
  const [isPending, startTransition] = useTransition();
  const [portalTarget, setPortalTarget] = useState<HTMLElement | null>(null);
  const router = useRouter();
  const [autoTriggered, setAutoTriggered] = useState(false);
  const [autoOpen, setAutoOpen] = useState(false);

  useEffect(() => {
    if (typeof document !== 'undefined') {
      setPortalTarget(document.body);
    }
  }, []);

  useEffect(() => {
    if (typeof document === 'undefined') return;
    const shouldAutoOpen = document.cookie
      .split(';')
      .map((c) => c.trim())
      .some((c) => c.startsWith('pb-auto-open-create=1'));
    setAutoOpen(shouldAutoOpen);
    if (shouldAutoOpen) {
      setOpen(true);
    }
  }, []);

  useEffect(() => {
    if (typeof document === 'undefined') return;
    const provided = document.cookie
      .split(';')
      .map((c) => c.trim())
      .some((c) => c.startsWith('pb-manager-token='));
    setHasProvidedToken(provided);
  }, []);

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
        // Prefer internal API when a provided token is present to avoid CORS/Gate flake.
        if (hasProvidedToken) {
          const resp = await fetch('/api/test/create-beach', {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ name, slug }),
          });
          if (resp.ok) {
            const data = await resp.json();
            const id = data.id as string;
            debug('internal create success', { id });
            handleOpenChange(false, 'create-success');
            router.push(`/beaches/${id}`);
            return;
          }
          const detail = await resp.text();
          debug('internal create failed', detail);
        }

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

  useEffect(() => {
    if (!autoOpen || autoTriggered) return;
    setAutoTriggered(true);
    handleCreate();
  }, [autoOpen, autoTriggered]);

  const modalContent = !open
    ? null
    : (
        <div
          data-testid="create-beach-modal"
          className="fixed inset-0 z-50 flex items-center justify-center p-4"
          role="dialog"
          aria-modal="true"
          aria-labelledby="create-beach-dialog-title"
          aria-describedby="create-beach-dialog-description"
        >
          <div
            className="absolute inset-0 bg-black/50 backdrop-blur-sm transition-opacity dark:bg-black/70"
            onClick={() => handleOpenChange(false, 'backdrop-click')}
          />
          <div className="relative z-10 w-full max-w-md rounded-lg border border-border bg-card text-card-foreground shadow-xl">
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
      );

  return (
    <>
      <Button size="sm" onClick={() => handleOpenChange(true, 'trigger-click')}>
        New Beach
      </Button>
      {modalContent
        ? portalTarget
          ? createPortal(modalContent, portalTarget)
          : modalContent
        : null}
    </>
  );
}
