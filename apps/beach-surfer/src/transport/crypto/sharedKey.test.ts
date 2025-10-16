import { describe, it, expect } from 'vitest';
import { derivePreSharedKey, toHex } from './sharedKey';

describe('derivePreSharedKey', () => {
  it(
    'matches Rust Argon2id output for known vector',
    async () => {
      const key = await derivePreSharedKey('correct horse battery staple', 'session-id-1234');
      expect(toHex(key)).toBe('939fee58f639eab75e07d235688001ebba6a2146b95c4ee013961819847943fc');
    },
    20_000,
  );
});
