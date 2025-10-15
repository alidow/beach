import { chacha20poly1305 } from '@noble/ciphers/chacha';

const TAG_SIZE = 16;

export function chacha20poly1305Encrypt(
  key: Uint8Array,
  nonce: Uint8Array,
  aad: Uint8Array,
  plaintext: Uint8Array,
): Uint8Array {
  const aead = chacha20poly1305(key, nonce, aad);
  return aead.encrypt(plaintext);
}

export function chacha20poly1305Decrypt(
  key: Uint8Array,
  nonce: Uint8Array,
  aad: Uint8Array,
  ciphertextAndTag: Uint8Array,
): Uint8Array {
  if (ciphertextAndTag.length < TAG_SIZE) {
    throw new Error('ciphertext too short');
  }

  const aead = chacha20poly1305(key, nonce, aad);

  try {
    return aead.decrypt(ciphertextAndTag);
  } catch (error) {
    if (
      error instanceof Error &&
      (error.message === 'invalid tag' || error.message.includes('invalid tag'))
    ) {
      throw new Error('authentication tag mismatch');
    }
    throw error;
  }
}
