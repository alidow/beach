'use client';

import { useCallback, useEffect, useMemo, useRef } from 'react';
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
  const tileStateRef = useRef(tileState);
  tileStateRef.current = tileState;
  const signatureRef = useRef(signature);
  signatureRef.current = signature;
  const baseLayoutRef = useRef<CanvasLayout | null>(initialLayout ?? null);
  const lastSavedSignatureRef = useRef<string>(initialSignature ?? '');
  const pendingSignatureRef = useRef<string | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    baseLayoutRef.current = initialLayout ?? baseLayoutRef.current;
  }, [initialLayout]);

  useEffect(() => {
    if (initialSignature) {
      lastSavedSignatureRef.current = initialSignature;
    }
  }, [initialSignature]);

  const performPersist = useCallback(
    async (sig: string) => {
      if (!beachId || !managerUrl || !managerToken) {
        return;
      }
      const previousLayout = baseLayoutRef.current;
      const nextLayout = tileStateToLayout(tileStateRef.current, previousLayout ?? undefined);
      baseLayoutRef.current = nextLayout;
      try {
        const saved = await putCanvasLayout(beachId, nextLayout, managerToken, managerUrl);
        baseLayoutRef.current = saved;
        lastSavedSignatureRef.current = sig;
        pendingSignatureRef.current = null;
      } catch (error) {
        console.warn('[rewrite] failed to persist canvas layout', {
          beachId,
          error: error instanceof Error ? error.message : String(error),
        });
        baseLayoutRef.current = previousLayout ?? null;
        pendingSignatureRef.current = null;
      }
    },
    [beachId, managerToken, managerUrl],
  );

  const schedulePersist = useCallback(
    (sig: string, options?: { immediate?: boolean }) => {
      if (sig.length === 0) {
        return;
      }
      if (!options?.immediate) {
        if (sig === lastSavedSignatureRef.current || sig === pendingSignatureRef.current) {
          return;
        }
        pendingSignatureRef.current = sig;
        if (timerRef.current) {
          clearTimeout(timerRef.current);
        }
        timerRef.current = setTimeout(() => {
          timerRef.current = null;
          void performPersist(sig);
        }, debounceMs);
        return;
      }

      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      pendingSignatureRef.current = sig;
      void performPersist(sig);
    },
    [debounceMs, performPersist],
  );

  useEffect(() => {
    schedulePersist(signature);
  }, [schedulePersist, signature]);

  const requestImmediatePersist = useCallback(() => {
    const sig = signatureRef.current;
    if (typeof window === 'undefined') {
      schedulePersist(sig, { immediate: true });
      return;
    }
    requestAnimationFrame(() => {
      schedulePersist(sig, { immediate: true });
    });
  }, [schedulePersist]);

  useEffect(() => {
    const handler = () => {
      schedulePersist(signatureRef.current, { immediate: true });
    };
    window.addEventListener('beforeunload', handler);
    return () => {
      window.removeEventListener('beforeunload', handler);
    };
  }, [schedulePersist]);

  return requestImmediatePersist;
}
