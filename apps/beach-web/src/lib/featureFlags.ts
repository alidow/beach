export type AppVariant = 'legacy' | 'v2';

const V2_MATCHERS = new Set(['v2', 'new', 'next', 'terminal']);
const LEGACY_MATCHERS = new Set(['legacy', 'v1', 'classic']);

function normalise(value: string | null | undefined): string | null {
  if (value == null) {
    return null;
  }
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  return trimmed.toLowerCase();
}

function parseVariant(value: string | null | undefined): AppVariant | null {
  const normalised = normalise(value);
  if (!normalised) {
    return null;
  }
  if (V2_MATCHERS.has(normalised)) {
    return 'v2';
  }
  if (LEGACY_MATCHERS.has(normalised)) {
    return 'legacy';
  }
  return null;
}

function queryVariant(): AppVariant | null {
  if (typeof window === 'undefined') {
    return null;
  }
  try {
    const params = new URLSearchParams(window.location.search);
    return parseVariant(params.get('ui') ?? params.get('variant'));
  } catch (err) {
    return null;
  }
}

function envVariant(): AppVariant | null {
  const envValue = (import.meta.env as Record<string, string | undefined>).VITE_BEACH_WEB_UI;
  return parseVariant(envValue);
}

function pathVariant(): AppVariant | null {
  if (typeof window === 'undefined') {
    return null;
  }
  const pathname = window.location.pathname.toLowerCase();
  if (pathname.startsWith('/v2')) {
    return 'v2';
  }
  return null;
}

export function resolveAppVariant(): AppVariant {
  return queryVariant() ?? envVariant() ?? pathVariant() ?? 'legacy';
}

export function shouldUseAppV2(): boolean {
  return resolveAppVariant() === 'v2';
}
