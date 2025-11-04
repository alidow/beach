'use client';

import { useAuth } from '@clerk/nextjs';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useInitialManagerToken } from './ManagerTokenContext';

type ManagerTokenState = {
  token: string | null;
  loading: boolean;
  error: string | null;
  isLoaded: boolean;
  isSignedIn: boolean;
  refresh: () => Promise<string | null>;
};

export function useManagerToken(): ManagerTokenState {
  const { isLoaded, isSignedIn, getToken } = useAuth();
  const { initialToken } = useInitialManagerToken();

  const fallbackToken = useMemo(() => {
    const fromContext = initialToken?.trim() ?? '';
    if (fromContext.length > 0) {
      return fromContext;
    }
    const fromEnv = process.env.NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_TOKEN?.trim() ?? '';
    if (fromEnv.length > 0) {
      return fromEnv;
    }
    return null;
  }, [initialToken]);

  const [token, setToken] = useState<string | null>(fallbackToken);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const template = process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE;

  const refresh = useCallback(async () => {
    if (fallbackToken) {
      setToken(fallbackToken);
      setError(null);
      return fallbackToken;
    }
    if (!isLoaded) {
      setToken(null);
      setError('Authentication is still loading.');
      return null;
    }
    if (!isSignedIn) {
      setToken(null);
      setError('Sign in to manage sessions.');
      return null;
    }

    setLoading(true);
    setError(null);
    try {
      const next = await getToken(template ? { template } : undefined);
      const trimmed = next?.trim() ?? null;
      setToken(trimmed && trimmed.length > 0 ? trimmed : null);
      return trimmed && trimmed.length > 0 ? trimmed : null;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setToken(null);
      setError(message);
      return null;
    } finally {
      setLoading(false);
    }
  }, [fallbackToken, getToken, isLoaded, isSignedIn, template]);

  useEffect(() => {
    if (fallbackToken) {
      setToken(fallbackToken);
      setError(null);
      return;
    }
    if (!isLoaded) {
      return;
    }
    void refresh();
  }, [fallbackToken, isLoaded, refresh]);

  return useMemo(
    () => ({
      token,
      loading,
      error,
      isLoaded: Boolean(fallbackToken) || isLoaded,
      isSignedIn: Boolean(fallbackToken) || isSignedIn,
      refresh,
    }),
    [token, loading, error, fallbackToken, isLoaded, isSignedIn, refresh],
  );
}

export function buildManagerUrl(fallback?: string | null): string {
  const fromPublic = process.env.NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_URL?.trim();
  if (fromPublic && fromPublic.length > 0) {
    return fromPublic;
  }
  const fromLegacyPublic = process.env.NEXT_PUBLIC_MANAGER_URL?.trim();
  if (fromLegacyPublic && fromLegacyPublic.length > 0) {
    return fromLegacyPublic;
  }
  const normalizedFallback = fallback?.trim() ?? '';
  if (normalizedFallback.length > 0) {
    return normalizedFallback;
  }
  const privateEnv = process.env.PRIVATE_BEACH_MANAGER_URL?.trim();
  if (privateEnv && privateEnv.length > 0) {
    return privateEnv;
  }
  return 'http://localhost:8080';
}
