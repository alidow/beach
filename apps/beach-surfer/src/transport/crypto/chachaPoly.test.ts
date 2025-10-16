import { describe, expect, it } from 'vitest';

import { chacha20poly1305Decrypt, chacha20poly1305Encrypt } from './chachaPoly';

function hexToBytes(hex: string): Uint8Array {
  if (hex.length % 2 !== 0) {
    throw new Error('invalid hex');
  }
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

describe('chacha20poly1305', () => {
  const key = hexToBytes('000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f');
  const nonce = hexToBytes('000000000000004a00000000');
  const aad = hexToBytes('f33388860000000000004e91');
  const plaintext = hexToBytes(
    '496e7465726e65742d447261667473206172652064726166747320776869636820636f6d70726973652074686520696e666f726d6174696f6e',
  );

  it('encrypts and decrypts round-trip', () => {
    const ciphertext = chacha20poly1305Encrypt(key, nonce, aad, plaintext);
    const decrypted = chacha20poly1305Decrypt(key, nonce, aad, ciphertext);
    expect(Array.from(decrypted)).toEqual(Array.from(plaintext));
  });
});
