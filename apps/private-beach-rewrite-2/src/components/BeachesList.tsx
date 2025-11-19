'use client';

import Link from 'next/link';
import { useMemo, useState, useTransition } from 'react';
import { useRouter } from 'next/navigation';
import type { BeachSummary } from '@/lib/api';
import { deleteBeachesAction } from '@/app/beaches/actions';
import { Input } from '../../../private-beach/src/components/ui/input';
import { Card, CardContent, CardHeader } from '../../../private-beach/src/components/ui/card';
import { Button } from '../../../private-beach/src/components/ui/button';

const createdAtFormatter = typeof Intl !== 'undefined'
  ? new Intl.DateTimeFormat('en-US', { dateStyle: 'medium', timeStyle: 'short', timeZone: 'UTC' })
  : null;

const formatCreatedAt = (value: string | number | Date) => {
  const date = value instanceof Date ? value : new Date(value);
  if (!createdAtFormatter) {
    return date.toISOString();
  }
  try {
    return createdAtFormatter.format(date);
  } catch {
    return date.toISOString();
  }
};

type Props = {
  beaches: BeachSummary[];
  error?: string | null;
};

export function BeachesList({ beaches, error }: Props) {
  const router = useRouter();
  const [query, setQuery] = useState('');
  const [selectedIds, setSelectedIds] = useState<Set<string>>(() => new Set());
  const [actionError, setActionError] = useState<string | null>(null);
  const [isPending, startTransition] = useTransition();

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

  const selectedCount = selectedIds.size;
  const selectedLabel = selectedCount === 0
    ? 'No beaches selected'
    : `${selectedCount} beach${selectedCount === 1 ? '' : 'es'} selected`;
  const allVisibleSelected = filtered.length > 0 && filtered.every((beach) => selectedIds.has(beach.id));

  const toggleSelection = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const toggleVisibleSelection = () => {
    if (filtered.length === 0) {
      return;
    }
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (allVisibleSelected) {
        filtered.forEach((beach) => next.delete(beach.id));
      } else {
        filtered.forEach((beach) => next.add(beach.id));
      }
      return next;
    });
  };

  const clearSelection = () => {
    if (selectedCount === 0) {
      return;
    }
    setSelectedIds(() => new Set());
  };

  const performDelete = (ids: string[]) => {
    if (ids.length === 0) {
      return;
    }
    const confirmation = ids.length === 1
      ? 'Delete this beach? All Beach Manager connections will be terminated.'
      : `Delete ${ids.length} beaches? All Beach Manager connections will be terminated.`;
    if (typeof window !== 'undefined' && !window.confirm(confirmation)) {
      return;
    }
    setActionError(null);
    startTransition(async () => {
      try {
        const result = await deleteBeachesAction({ ids });
        if (result.success) {
          setSelectedIds((prev) => {
            const next = new Set(prev);
            result.deleted.forEach((id) => next.delete(id));
            return next;
          });
          router.refresh();
        } else {
          setActionError(result.error);
        }
      } catch (actionErr) {
        const message = actionErr instanceof Error ? actionErr.message : String(actionErr);
        setActionError(message);
      }
    });
  };

  const handleQuickDelete = (id: string) => performDelete([id]);
  const handleBulkDelete = () => performDelete(Array.from(selectedIds));

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

  return (
    <div className="space-y-6">
      <div className="space-y-3">
        <Input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Search by name or id…"
        />
        <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
          <span>{selectedLabel}</span>
          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              type="button"
              onClick={toggleVisibleSelection}
              disabled={filtered.length === 0 || isPending}
            >
              {allVisibleSelected ? 'Deselect visible' : 'Select visible'}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              type="button"
              onClick={clearSelection}
              disabled={selectedCount === 0 || isPending}
            >
              Clear selection
            </Button>
            <Button
              variant="danger"
              size="sm"
              type="button"
              onClick={handleBulkDelete}
              disabled={selectedCount === 0 || isPending}
            >
              {isPending ? 'Deleting…' : selectedCount === 0 ? 'Delete selected' : `Delete ${selectedCount}`}
            </Button>
          </div>
        </div>
        {actionError ? (
          <div className="rounded border border-destructive/40 bg-destructive/10 p-2 text-xs text-destructive-foreground">
            {actionError}
          </div>
        ) : null}
      </div>
      {filtered.length === 0 ? (
        <div className="rounded-lg border border-dashed border-border p-6 text-sm text-muted-foreground">
          No beaches match your search. Create one to get started.
        </div>
      ) : (
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {filtered.map((beach) => {
            const isSelected = selectedIds.has(beach.id);
            return (
              <Card
                key={beach.id}
                className={isSelected ? 'border-primary shadow-md ring-1 ring-primary/30' : ''}
              >
                <CardHeader className={`flex flex-col gap-1 ${isSelected ? 'bg-primary/5' : ''}`}>
                  <div className="flex items-start justify-between gap-4">
                    <div className="flex items-start gap-3">
                      <input
                        type="checkbox"
                        className="mt-1 h-4 w-4 rounded border border-border text-primary focus:ring-2 focus:ring-primary/40"
                        checked={isSelected}
                        disabled={isPending}
                        onChange={() => toggleSelection(beach.id)}
                        aria-label={`Select ${beach.name}`}
                      />
                      <div>
                        <div className="text-sm font-semibold">{beach.name}</div>
                        <div className="text-xs text-muted-foreground">{beach.id}</div>
                      </div>
                    </div>
                    <div className="flex flex-wrap items-center gap-2">
                      <Link href={`/beaches/${beach.id}`}>
                        <Button size="sm" type="button" disabled={isPending}>
                          Open
                        </Button>
                      </Link>
                      <Link href={`/beaches/${beach.id}/settings`}>
                        <Button variant="outline" size="sm" type="button" disabled={isPending}>
                          Settings
                        </Button>
                      </Link>
                      <Button
                        variant="danger"
                        size="sm"
                        type="button"
                        onClick={() => handleQuickDelete(beach.id)}
                        disabled={isPending}
                      >
                        {isPending ? 'Deleting…' : 'Delete'}
                      </Button>
                    </div>
                  </div>
                </CardHeader>
                <CardContent className="text-xs text-muted-foreground">
                  Created {formatCreatedAt(beach.created_at)}
                </CardContent>
              </Card>
            );
          })}
        </div>
      )}
    </div>
  );
}
