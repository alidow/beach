'use client';

import Link from 'next/link';
import { useMemo, useState } from 'react';
import type { BeachSummary } from '@/lib/api';
import { Input } from '../../../private-beach/src/components/ui/input';
import { Card, CardContent, CardHeader } from '../../../private-beach/src/components/ui/card';
import { Button } from '../../../private-beach/src/components/ui/button';

type Props = {
  beaches: BeachSummary[];
  isSignedIn: boolean;
  error?: string | null;
};

export function BeachesList({ beaches, isSignedIn, error }: Props) {
  const [query, setQuery] = useState('');

  const filtered = useMemo(() => {
    const value = query.trim().toLowerCase();
    if (value.length === 0) {
      return beaches;
    }
    return beaches.filter((beach) => {
      const name = beach.name.toLowerCase();
      return name.includes(value) || beach.id.toLowerCase().includes(value);
    });
  }, [beaches, query]);

  if (error) {
    return (
      <div className="mx-auto max-w-2xl rounded-lg border border-destructive/40 bg-destructive/10 p-6 text-sm">
        <h2 className="text-sm font-semibold text-destructive-foreground">Unable to load beaches</h2>
        <p className="mt-2 text-destructive-foreground/80">
          {error}
        </p>
      </div>
    );
  }

  if (!isSignedIn) {
    return (
      <div className="mx-auto max-w-2xl rounded-lg border border-dashed border-border bg-background/60 p-8 text-center">
        <h2 className="text-lg font-semibold">You need to sign in</h2>
        <p className="mt-2 text-sm text-muted-foreground">
          Sign in to view your private beaches and continue with the rewrite workspace.
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div>
        <Input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Search by name or id…"
        />
      </div>
      {filtered.length === 0 ? (
        <div className="rounded-lg border border-dashed border-border p-6 text-sm text-muted-foreground">
          No beaches match your search. Create one to get started.
        </div>
      ) : (
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {filtered.map((beach) => (
            <Card key={beach.id}>
              <CardHeader className="flex flex-col gap-1">
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <div className="text-sm font-semibold">{beach.name}</div>
                    <div className="text-xs text-muted-foreground">{beach.id}</div>
                  </div>
                  <div className="flex items-center gap-2">
                    <Link href={`/beaches/${beach.id}`}>
                      <Button size="sm">Open</Button>
                    </Link>
                    <Link href={`/beaches/${beach.id}/settings`}>
                      <Button variant="outline" size="sm">
                        Settings
                      </Button>
                    </Link>
                  </div>
                </div>
              </CardHeader>
              <CardContent className="text-xs text-muted-foreground">
                Created {new Date(beach.created_at).toLocaleString()}
              </CardContent>
            </Card>
          ))}
        </div>
      )}
    </div>
  );
}
