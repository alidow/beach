import { URL } from 'url';

type ClerkGetTokenFn =
  | ((options?: { template?: string; skipCache?: boolean }) => Promise<string | null>)
  | undefined;

function resolveGateUrl(): string {
  const privateUrl = process.env.PRIVATE_BEACH_GATE_URL;
  if (privateUrl && privateUrl.trim().length > 0) {
    return privateUrl.trim();
  }
  const publicUrl = process.env.NEXT_PUBLIC_PRIVATE_BEACH_GATE_URL ?? process.env.BEACH_GATE_URL;
  if (publicUrl && publicUrl.trim().length > 0) {
    return publicUrl.trim();
  }
  const inferred = inferGateFromManagerUrl();
  if (inferred) {
    return inferred;
  }
  return 'http://localhost:4133';
}

function inferGateFromManagerUrl(): string | null {
  const managerUrl =
    process.env.PRIVATE_BEACH_MANAGER_URL ??
    process.env.NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_URL ??
    process.env.NEXT_PUBLIC_MANAGER_URL;
  if (!managerUrl) {
    return null;
  }
  try {
    const parsed = new URL(managerUrl);
    if (parsed.hostname === 'beach-manager') {
      return 'http://beach-gate:4133';
    }
  } catch {
    // ignore parse errors
  }
  return null;
}

function resolveEnvToken(): string | null {
  const direct = process.env.PRIVATE_BEACH_MANAGER_TOKEN;
  if (direct && direct.trim().length > 0) {
    return direct.trim();
  }
  const legacy = process.env.PRIVATE_BEACH_MANAGER_JWT;
  if (legacy && legacy.trim().length > 0) {
    return legacy.trim();
  }
  return null;
}

async function exchangeClerkTokenForGateToken(clerkToken: string): Promise<string | null> {
  const gateUrl = resolveGateUrl();
  const url = new URL('/auth/exchange', gateUrl);
  const response = await fetch(url, {
    method: 'POST',
    headers: {
      authorization: `Bearer ${clerkToken}`,
    },
    cache: 'no-store',
  });

  if (!response.ok) {
    const detail = await response.text().catch(() => response.statusText);
    throw new Error(`gate_exchange_failed:${response.status}:${detail}`);
  }

  const body = (await response.json()) as { access_token?: string };
  const token = body.access_token?.trim() ?? '';
  return token.length > 0 ? token : null;
}

export type ManagerTokenResolution = {
  token: string | null;
  source: 'env' | 'exchange' | 'unauthenticated' | 'exchange_error' | 'none';
  detail?: string;
};

type ResolveOptions = {
  isAuthenticated?: boolean;
};

export async function resolveManagerToken(
  getToken: ClerkGetTokenFn,
  template: string | undefined,
  options?: ResolveOptions,
): Promise<ManagerTokenResolution> {
  const envToken = resolveEnvToken();
  if (envToken) {
    return { token: envToken, source: 'env' };
  }

  const isAuthenticated = options?.isAuthenticated ?? true;
  if (!isAuthenticated) {
    return { token: null, source: 'unauthenticated' };
  }

  if (typeof getToken === 'function') {
    try {
      const clerkToken = await getToken(template ? { template, skipCache: true } : { skipCache: true });
      const trimmed = clerkToken?.trim() ?? '';
      if (trimmed.length > 0) {
        const gateToken = await exchangeClerkTokenForGateToken(trimmed);
        if (gateToken) {
          return { token: gateToken, source: 'exchange' };
        }
        return { token: null, source: 'exchange_error', detail: 'gate_token_missing' };
      }
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      return { token: null, source: 'exchange_error', detail };
    }
  }

  return { token: null, source: 'none' };
}

export function resolveManagerBaseUrl(): string {
  const privateUrl = process.env.PRIVATE_BEACH_MANAGER_URL;
  if (privateUrl && privateUrl.trim().length > 0) {
    return privateUrl.trim();
  }
  const publicUrl = process.env.NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_URL ?? process.env.NEXT_PUBLIC_MANAGER_URL;
  if (publicUrl && publicUrl.trim().length > 0) {
    return publicUrl.trim();
  }
  return 'http://localhost:8080';
}
