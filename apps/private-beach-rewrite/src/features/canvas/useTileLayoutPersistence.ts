'use client';

import { useEffect, useMemo, useRef } from 'react';
import { putCanvasLayout, type CanvasLayout } from '@/lib/api';
import { serializeTileStateKey, tileStateToLayout } from '@/features/tiles/persistence';
import { useTileState } from '@/features/tiles/store';
import { useManagerToken } from '@/hooks/useManagerToken';

type PersistenceOptions = {
  beachId: string;
  managerUrl?: string;
  debounceMs?: number;
  initialLayout?: CanvasLayout | null;
  initialSignature?: string;
};

export function useTileLayoutPersistence({
  beachId,
  managerUrl,
  debounceMs = 200,
  initialLayout,
  initialSignature,
}: PersistenceOptions) {
  const tileState = useTileState();
  const { token: managerToken } = useManagerToken();
  const signature = useMemo(() => serializeTileStateKey(tileState), [tileState]);
  const baseLayoutRef = useRef<CanvasLayout | null>(initialLayout ?? null);
  const lastSavedSignatureRef = useRef<string>(initialSignature ?? '');
  const pendingSignatureRef = useRef<string | null>(null);

  useEffect(() => {
    baseLayoutRef.current = initialLayout ?? baseLayoutRef.current;
  }, [initialLayout]);

  useEffect(() => {
    if (initialSignature) {
      lastSavedSignatureRef.current = initialSignature;
    }
  }, [initialSignature]);

  useEffect(() => {
    if (!beachId || !managerUrl || !managerToken) {
      return;
    }
    if (signature.length === 0) {
      return;
    }
    if (signature === lastSavedSignatureRef.current || signature === pendingSignatureRef.current) {
      return;
    }

    pendingSignatureRef.current = signature;

    const handle = setTimeout(() => {
      const previousLayout = baseLayoutRef.current;
      const nextLayout = tileStateToLayout(tileState, previousLayout ?? undefined);
      baseLayoutRef.current = nextLayout;
      void putCanvasLayout(beachId, nextLayout, managerToken, managerUrl)
        .then((saved) => {
          baseLayoutRef.current = saved;
          lastSavedSignatureRef.current = signature;
          pendingSignatureRef.current = null;
        })
        .catch((error) => {
          console.warn('[rewrite] failed to persist canvas layout', {
            beachId,
            error: error instanceof Error ? error.message : String(error),
          });
          baseLayoutRef.current = previousLayout ?? null;
          pendingSignatureRef.current = null;
        });
    }, debounceMs);

    return () => {
      clearTimeout(handle);
      if (pendingSignatureRef.current === signature) {
        pendingSignatureRef.current = null;
      }
    };
  }, [signature, beachId, managerUrl, managerToken, debounceMs, tileState]);
}
