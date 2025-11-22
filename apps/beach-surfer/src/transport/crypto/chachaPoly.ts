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
      logTrace('authentication_tag_mismatch', {
        key_fingerprint: fingerprint(key),
        nonce: toHex(nonce),
        aad_len: aad.length,
        ciphertext_len: ciphertextAndTag.length,
      });
      throw new Error('authentication tag mismatch');
    }
    throw error;
  }
}

function fingerprint(bytes: Uint8Array): string {
  // Non-cryptographic fingerprint to correlate host/browser derivations without leaking the key.
  let hash = 0x811c9dc5 >>> 0;
  for (const byte of bytes) {
    hash ^= byte;
    hash = Math.imul(hash, 0x01000193) >>> 0; // 32-bit FNV-1a variant
  }
  return hash.toString(16).padStart(8, '0');
}

function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

function logTrace(event: string, data: Record<string, unknown>): void {
  const host = globalThis as { __BEACH_TRACE?: boolean; BEACH_TRACE?: boolean } | undefined;
  const enabled = Boolean(host?.__BEACH_TRACE ?? host?.BEACH_TRACE);
  if (!enabled) return;
  // eslint-disable-next-line no-console
  console.warn('[beach-surfer][crypto]', event, data);
}
