/**
 * Shared-secret derivation helpers for sealed signaling and post-handshake crypto.
 *
 * NOTE: We will swap the PBKDF2 placeholder for Argon2id once the WASM dependency
 * is added to the bundle. The interim implementation keeps the API stable so the
 * rest of the pipeline can be wired up safely.
 */

const DERIVED_KEY_LENGTH_BITS = 32 * 8;

/**
 * Stretch a human passphrase using PBKDF2(SHA-256) while we integrate Argon2id.
 * Salt is expected to be the handshake identifier so both peers converge on the
 * same 32-byte secret.
 */
export async function derivePreSharedKey(
  passphrase: string,
  salt: string,
): Promise<Uint8Array> {
  const encoder = new TextEncoder();
  const passphraseBytes = encoder.encode(passphrase);
  const saltBytes = encoder.encode(salt);

  const keyMaterial = await crypto.subtle.importKey(
    'raw',
    passphraseBytes,
    { name: 'PBKDF2' },
    false,
    ['deriveBits'],
  );

  const derivedBits = await crypto.subtle.deriveBits(
    {
      name: 'PBKDF2',
      hash: 'SHA-256',
      iterations: 64_000,
      salt: saltBytes,
    },
    keyMaterial,
    DERIVED_KEY_LENGTH_BITS,
  );

  return new Uint8Array(derivedBits);
}

/**
 * Expand a stretched key into context-specific material using HKDF-SHA256.
 */
export async function hkdfExpand(
  ikm: Uint8Array,
  salt: Uint8Array,
  info: Uint8Array,
  length: number,
): Promise<Uint8Array> {
  const keyMaterial = await crypto.subtle.importKey(
    'raw',
    ikm,
    'HKDF',
    false,
    ['deriveBits'],
  );

  const derivedBits = await crypto.subtle.deriveBits(
    {
      name: 'HKDF',
      hash: 'SHA-256',
      salt,
      info,
    },
    keyMaterial,
    length * 8,
  );

  return new Uint8Array(derivedBits);
}

export function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}
