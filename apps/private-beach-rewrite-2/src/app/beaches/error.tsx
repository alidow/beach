'use client';

import { useEffect } from 'react';
import { AppShellTopNav } from '@/components/AppShellTopNav';

type Props = {
  error: Error & { digest?: string };
  reset: () => void;
};

export default function BeachesError({ error, reset }: Props) {
  useEffect(() => {
    console.error('[ws-b] beaches error boundary', error);
  }, [error]);

  return (
    <div className="flex min-h-screen flex-col bg-background">
      <AppShellTopNav title="Private Beach" subtitle="We hit a snag loading your beaches." />
      <main className="flex-1">
        <div className="mx-auto flex h-full max-w-3xl flex-col items-center justify-center gap-4 px-4 text-center sm:px-6 lg:px-8">
          <div className="rounded-lg border border-destructive/40 bg-destructive/10 px-6 py-4 text-sm text-destructive-foreground">
            {error.message || 'An unexpected error occurred while loading your beaches.'}
          </div>
          <button
            type="button"
            className="rounded-md border border-border bg-background px-4 py-2 text-sm font-medium text-foreground shadow-sm hover:bg-muted"
            onClick={() => reset()}
          >
            Try again
          </button>
        </div>
      </main>
    </div>
  );
}
