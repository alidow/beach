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

const TOKEN_REFRESH_SKEW_MS = 30_000;

function base64UrlDecode(segment: string): string | null {
  if (!segment) return null;
  const normalized = segment.replace(/-/g, '+').replace(/_/g, '/');
  const padding = normalized.length % 4 === 0 ? '' : '='.repeat(4 - (normalized.length % 4));
  try {
    if (typeof window !== 'undefined' && typeof window.atob === 'function') {
      return window.atob(normalized + padding);
    }
  } catch {
    return null;
  }
  return null;
}

function decodeJwtClaims(token: string | null): Record<string, unknown> | null {
  if (!token) return null;
  const parts = token.split('.');
  if (parts.length < 2) {
    return null;
  }
  const decoded = base64UrlDecode(parts[1]);
  if (!decoded) {
    return null;
  }
  try {
    return JSON.parse(decoded) as Record<string, unknown>;
  } catch {
    return null;
  }
}

function getTokenExpiryMs(token: string | null): number | null {
  if (!token) return null;
  const claims = decodeJwtClaims(token);
  const expSeconds = claims && typeof claims.exp === 'number' ? (claims.exp as number) : null;
  return expSeconds ? expSeconds * 1000 : null;
}

type TokenLogContext = Record<string, unknown> & { phase: string };

function logTokenDiagnostics(token: string | null, context: TokenLogContext) {
  const payload: Record<string, unknown> = {
    ...context,
    hasToken: Boolean(token && token.trim().length > 0),
  };
  if (token && token.trim().length > 0) {
    const claims = (decodeJwtClaims(token) ?? {}) as Record<string, unknown>;
    const scope = typeof claims['scope'] === 'string' ? (claims['scope'] as string) : null;
    const scp = Array.isArray(claims['scp']) ? (claims['scp'] as unknown[]) : null;
    const entitlements = claims['entitlements'] ?? null;
    payload.scope = scope;
    payload.scp = scp;
    payload.entitlements = entitlements;
    payload.exp = getTokenExpiryMs(token);
    payload.tokenPrefix = token.slice(0, 12);
  }
  try {
    console.info('[rewrite-2][auth] manager-token', JSON.stringify(payload));
  } catch {
    console.info('[rewrite-2][auth] manager-token', payload);
  }
}

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
  const [lastRefreshId, setLastRefreshId] = useState<number>(0);

  const refresh = useCallback(async () => {
    if (!isLoaded) {
      setToken(null);
      setError('Authentication is still loading.');
      logTokenDiagnostics(null, { phase: 'refresh-skipped', reason: 'auth-loading' });
      return null;
    }
    if (!isSignedIn) {
      setToken(null);
      setError('Sign in to manage sessions.');
      logTokenDiagnostics(null, { phase: 'refresh-skipped', reason: 'signed-out' });
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
      setLastRefreshId((current) => current + 1);
      logTokenDiagnostics(nextToken, { phase: 'refresh-success', source: 'api/manager-token' });
      return nextToken;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setToken(null);
      setError(message);
      logTokenDiagnostics(null, { phase: 'refresh-error', error: message });
      return null;
    } finally {
      setLoading(false);
    }
  }, [isLoaded, isSignedIn]);

  useEffect(() => {
    if (fallbackToken) {
      setToken(fallbackToken);
      setError(null);
      setLastRefreshId((current) => current + 1);
      logTokenDiagnostics(fallbackToken, { phase: 'fallback-init' });
      return;
    }
    if (!isLoaded) {
      return;
    }
    void refresh();
  }, [fallbackToken, isLoaded, refresh]);

  useEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }
    if (!token) {
      return;
    }
    const expiryMs = getTokenExpiryMs(token);
    if (!expiryMs) {
      return;
    }
    const delay = Math.max(1_000, expiryMs - TOKEN_REFRESH_SKEW_MS - Date.now());
    if (delay <= 0) {
      void refresh();
      return;
    }
    const timer = window.setTimeout(() => {
      void refresh();
    }, delay);
    return () => window.clearTimeout(timer);
  }, [token, refresh, lastRefreshId]);

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
