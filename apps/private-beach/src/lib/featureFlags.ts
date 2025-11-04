const STORAGE_KEY = 'private-beach-rewrite';

const TRUTHY = new Set(['1', 'true', 'yes', 'on', 'enable', 'enabled']);
const FALSY = new Set(['0', 'false', 'no', 'off', 'disable', 'disabled']);

type StorageLike = Pick<Storage, 'getItem'> & Partial<Pick<Storage, 'setItem'>>;

type ResolveOptions = {
  env?: string | null;
  search?: string | null;
  storage?: StorageLike | null;
};

function parseBooleanFlag(value: string | null | undefined): boolean | null {
  if (typeof value !== 'string') return null;
  const normalized = value.trim().toLowerCase();
  if (normalized.length === 0) return null;
  if (TRUTHY.has(normalized)) return true;
  if (FALSY.has(normalized)) return false;
  return null;
}

function coerceSearchFlag(search: string | null | undefined): boolean | null {
  if (!search) return null;
  try {
    const normalized = search.startsWith('?') ? search : `?${search}`;
    const params = new URLSearchParams(normalized);
    const candidates = [
      params.get('rewrite'),
      params.get('privateBeachRewrite'),
      params.get('private_beach_rewrite'),
      params.get('pbRewrite'),
    ];
    for (const candidate of candidates) {
      const parsed = parseBooleanFlag(candidate);
      if (parsed !== null) {
        return parsed;
      }
    }
  } catch {
    // ignore malformed search strings
  }
  return null;
}

export function resolvePrivateBeachRewriteEnabled(options: ResolveOptions = {}): boolean {
  const envValue =
    parseBooleanFlag(
      options.env ??
        (typeof process !== 'undefined'
          ? (process.env?.NEXT_PUBLIC_PRIVATE_BEACH_REWRITE_ENABLED as string | undefined | null)
          : null),
    ) ?? false;

  let result = envValue;
  const queryValue = coerceSearchFlag(options.search ?? null);
  if (queryValue !== null) {
    result = queryValue;
  }

  const storage = options.storage ?? (typeof window !== 'undefined' ? window.localStorage ?? null : null);
  if (storage?.getItem) {
    try {
      const stored = storage.getItem(STORAGE_KEY);
      const storedValue = parseBooleanFlag(stored);
      if (storedValue !== null) {
        result = storedValue;
      }
    } catch {
      // ignore storage access issues (e.g., privacy mode)
    }
  }

  return result;
}

export function isPrivateBeachRewriteEnabled(): boolean {
  if (typeof window === 'undefined') {
    return resolvePrivateBeachRewriteEnabled();
  }
  return resolvePrivateBeachRewriteEnabled({
    search: window.location.search,
    storage: window.localStorage ?? null,
  });
}

export function rememberPrivateBeachRewritePreference(
  enabled: boolean,
  storage: StorageLike | null | undefined = typeof window !== 'undefined' ? window.localStorage : null,
): void {
  if (!storage?.setItem) {
    return;
  }
  try {
    storage.setItem(STORAGE_KEY, enabled ? '1' : '0');
  } catch {
    // ignore storage failures
  }
}

export { STORAGE_KEY as PRIVATE_BEACH_REWRITE_STORAGE_KEY };
