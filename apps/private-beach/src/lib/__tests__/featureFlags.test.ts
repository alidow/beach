import { describe, it, expect } from 'vitest';
import {
  resolvePrivateBeachRewriteEnabled,
  rememberPrivateBeachRewritePreference,
  PRIVATE_BEACH_REWRITE_STORAGE_KEY,
} from '../featureFlags';

type MockStorage = {
  data: Map<string, string>;
  getItem: (key: string) => string | null;
  setItem: (key: string, value: string) => void;
};

function createStorage(seed: Record<string, string> = {}): MockStorage {
  const data = new Map(Object.entries(seed));
  return {
    data,
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => {
      data.set(key, value);
    },
  };
}

describe('resolvePrivateBeachRewriteEnabled', () => {
  it('defaults to false when no sources are provided', () => {
    expect(resolvePrivateBeachRewriteEnabled()).toBe(false);
  });

  it('honours environment defaults', () => {
    expect(resolvePrivateBeachRewriteEnabled({ env: 'true' })).toBe(true);
    expect(resolvePrivateBeachRewriteEnabled({ env: 'false' })).toBe(false);
  });

  it('allows query string overrides', () => {
    expect(resolvePrivateBeachRewriteEnabled({ env: 'false', search: '?rewrite=1' })).toBe(true);
    expect(resolvePrivateBeachRewriteEnabled({ env: 'true', search: 'rewrite=0' })).toBe(false);
  });

  it('storage overrides take precedence', () => {
    const storage = createStorage({ [PRIVATE_BEACH_REWRITE_STORAGE_KEY]: 'yes' });
    expect(resolvePrivateBeachRewriteEnabled({ env: 'false', storage })).toBe(true);
  });
});

describe('rememberPrivateBeachRewritePreference', () => {
  it('persists values into storage', () => {
    const storage = createStorage();
    rememberPrivateBeachRewritePreference(true, storage);
    expect(storage.getItem(PRIVATE_BEACH_REWRITE_STORAGE_KEY)).toBe('1');
    rememberPrivateBeachRewritePreference(false, storage);
    expect(storage.getItem(PRIVATE_BEACH_REWRITE_STORAGE_KEY)).toBe('0');
  });
});
