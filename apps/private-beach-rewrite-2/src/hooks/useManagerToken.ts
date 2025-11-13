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
  const { isLoaded, isSignedIn } = useAuth();
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

  const refresh = useCallback(async () => {
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
      const response = await fetch('/api/manager-token', {
        method: 'GET',
        credentials: 'include',
        cache: 'no-store',
      });
      if (!response.ok) {
        const detail = await response.text();
        throw new Error(detail || 'Unable to refresh manager token.');
      }
      const data = (await response.json()) as { token?: string | null };
      const nextToken = data.token?.trim() ?? null;
      setToken(nextToken);
      return nextToken;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setToken(null);
      setError(message);
      return null;
    } finally {
      setLoading(false);
    }
  }, [isLoaded, isSignedIn]);

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
      isSignedIn: Boolean(fallbackToken) || Boolean(isSignedIn),
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

export function buildRoadUrl(fallback?: string | null): string {
  const candidates = [
    process.env.NEXT_PUBLIC_PRIVATE_BEACH_ROAD_URL,
    process.env.NEXT_PUBLIC_ROAD_URL,
    process.env.NEXT_PUBLIC_SESSION_SERVER_URL,
    fallback,
    process.env.PRIVATE_BEACH_ROAD_URL,
  ];
  for (const candidate of candidates) {
    if (candidate && candidate.trim().length > 0) {
      return candidate.trim();
    }
  }
  throw new Error(
    'Beach Road URL is not configured. Set NEXT_PUBLIC_PRIVATE_BEACH_ROAD_URL (or add road_url in Private Beach settings).',
  );
}
