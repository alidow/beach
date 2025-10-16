import { deriveArgon2id } from './argon2';

const encoder = new TextEncoder();
const HANDSHAKE_INFO_BYTES = encoder.encode('beach:secure-signaling:handshake');

function isCryptoTraceEnabled(): boolean {
  if (typeof globalThis === 'undefined') {
    return false;
  }
  const host = globalThis as {
    __BEACH_TRACE?: boolean;
    BEACH_TRACE?: boolean;
  };
  const flag =
    host.__BEACH_TRACE ??
    host.BEACH_TRACE ??
    (typeof window !== 'undefined' ? window.__BEACH_TRACE ?? window.BEACH_TRACE : undefined);
  return Boolean(flag);
}

function logKeyTrace(event: string, data: Record<string, unknown>): void {
  if (!isCryptoTraceEnabled()) {
    return;
  }
  // eslint-disable-next-line no-console
  console.debug('[beach-surfer][crypto]', event, data);
}

async function truncatedHashHex(bytes: Uint8Array): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', toArrayBuffer(bytes)));
  return toHex(digest.subarray(0, 8));
}

/**
 * Shared-secret derivation helpers for sealed signaling and post-handshake crypto.
 */

/**
 * Stretch a human passphrase using Argon2id (aligned with the Rust toolchain).
 * Salt is expected to be the handshake identifier so both peers converge on the
 * same 32-byte secret.
 */
export async function derivePreSharedKey(
  passphrase: string,
  salt: string,
): Promise<Uint8Array> {
  try {
    const key = await deriveArgon2id({ passphrase, salt });
    logKeyTrace('session_key_derived', {
      session_id: salt,
      session_hash: await truncatedHashHex(key),
    });
    return key;
  } catch (error) {
    console.error('[beach-surfer] argon2 derive failed', error);
    throw error instanceof Error ? error : new Error(String(error));
  }
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
    toArrayBuffer(ikm),
    'HKDF',
    false,
    ['deriveBits'],
  );

  const derivedBits = await crypto.subtle.deriveBits(
    {
      name: 'HKDF',
      hash: 'SHA-256',
      salt: toArrayBuffer(salt),
      info: toArrayBuffer(info),
    },
    keyMaterial,
    length * 8,
  );

  return new Uint8Array(derivedBits);
}

export async function deriveHandshakeKey(
  sessionKey: Uint8Array,
  handshakeId: string,
): Promise<Uint8Array> {
  const salt = encoder.encode(handshakeId);
  const handshakeKey = await hkdfExpand(sessionKey, salt, HANDSHAKE_INFO_BYTES, 32);
  const [sessionHash, handshakeHash] = await Promise.all([
    truncatedHashHex(sessionKey),
    truncatedHashHex(handshakeKey),
  ]);
  logKeyTrace('handshake_key_derived', {
    handshake_id: handshakeId,
    session_hash: sessionHash,
    handshake_hash: handshakeHash,
  });
  return handshakeKey;
}

export function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

function toArrayBuffer(view: Uint8Array): ArrayBuffer {
  const { buffer, byteOffset, byteLength } = view;
  if (buffer instanceof ArrayBuffer) {
    if (byteOffset === 0 && byteLength === buffer.byteLength) {
      return buffer;
    }
    return buffer.slice(byteOffset, byteOffset + byteLength);
  }
  const copy = new Uint8Array(byteLength);
  copy.set(view);
  return copy.buffer;
}
