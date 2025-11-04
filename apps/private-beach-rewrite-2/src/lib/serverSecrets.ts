type ClerkGetTokenFn = ((options?: { template?: string }) => Promise<string | null>) | undefined;

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

export type ManagerTokenResolution = {
  token: string | null;
  source: 'env' | 'clerk' | 'none';
};

export async function resolveManagerToken(
  getToken: ClerkGetTokenFn,
  template: string | undefined,
): Promise<ManagerTokenResolution> {
  const envToken = resolveEnvToken();
  if (envToken) {
    return { token: envToken, source: 'env' };
  }

  if (typeof getToken === 'function') {
    try {
      const token = await getToken(template ? { template } : undefined);
      const trimmed = token?.trim() ?? '';
      if (trimmed.length > 0) {
        return { token: trimmed, source: 'clerk' };
      }
    } catch {
      // fall through to none
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

import { resolvePrivateBeachRewriteEnabled } from '../../../private-beach/src/lib/featureFlags';

type SearchParamsLike = URLSearchParams | Record<string, string | string[] | undefined> | undefined;

function stringifySearchParams(searchParams: SearchParamsLike): string | null {
  if (!searchParams) {
    return null;
  }
  if (searchParams instanceof URLSearchParams) {
    const serialized = searchParams.toString();
    return serialized.length > 0 ? `?${serialized}` : '';
  }
  const entries = Object.entries(searchParams).flatMap(([key, value]) => {
    if (typeof value === 'undefined') {
      return [];
    }
    if (Array.isArray(value)) {
      return value.map((item) => [key, item]);
    }
    return [[key, value]];
  });
  const params = new URLSearchParams();
  for (const [key, value] of entries) {
    if (typeof value === 'string') {
      params.append(key, value);
    }
  }
  const serialized = params.toString();
  return serialized.length > 0 ? `?${serialized}` : '';
}

export function resolveRewriteFlag(searchParams: SearchParamsLike): boolean {
  return resolvePrivateBeachRewriteEnabled({
    env: process.env.NEXT_PUBLIC_PRIVATE_BEACH_REWRITE_ENABLED ?? null,
    search: stringifySearchParams(searchParams),
  });
}

