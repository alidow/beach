import { useCallback, useEffect, useRef, useState } from 'react';

type RefreshOptions = {
  force?: boolean;
};

type ManagerTokenState = {
  token: string | null;
  loading: boolean;
  error: string | null;
  refresh: (options?: RefreshOptions) => Promise<string | null>;
};

async function fetchManagerToken(): Promise<string | null> {
  const response = await fetch('/api/manager-token', {
    method: 'GET',
    headers: {
      accept: 'application/json',
    },
    credentials: 'include',
  });
  if (!response.ok) {
    if (response.status === 401) {
      return null;
    }
    const detail = await response.json().catch(() => ({}));
    const message = typeof detail?.error === 'string' ? detail.error : `manager_token_error_${response.status}`;
    throw new Error(message);
  }
  const body = (await response.json()) as { token?: string };
  const token = body?.token?.trim() ?? '';
  return token.length > 0 ? token : null;
}

export function useManagerToken(enabled: boolean): ManagerTokenState {
  const [token, setToken] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inflightRef = useRef<Promise<string | null> | null>(null);

  const refresh = useCallback(async (options?: RefreshOptions) => {
    if (!enabled) {
      setToken(null);
      setError(null);
      return null;
    }
    if (inflightRef.current && !options?.force) {
      return inflightRef.current;
    }
    setLoading(true);
    const request = fetchManagerToken()
      .then((value) => {
        setToken(value);
        setError(null);
        setLoading(false);
        return value;
      })
      .catch((err: Error) => {
        setToken(null);
        setError(err.message);
        setLoading(false);
        return null;
      })
      .finally(() => {
        inflightRef.current = null;
      }) as Promise<string | null>;
    inflightRef.current = request;
    return request;
  }, [enabled]);

  useEffect(() => {
    if (!enabled) {
      setToken(null);
      setError(null);
      return;
    }
    refresh().catch(() => {});
  }, [enabled, refresh]);

  useEffect(() => {
    if (!enabled) {
      return;
    }
    const interval = setInterval(() => {
      refresh().catch(() => {});
    }, 60_000);
    return () => clearInterval(interval);
  }, [enabled, refresh]);

  return { token, loading, error, refresh };
}
