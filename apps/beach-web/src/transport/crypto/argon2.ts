/**
 * Thin wrapper around the noble-hashes Argon2 implementation that exposes an async helper
 * for deriving Argon2id hashes with the parameters used by the Rust toolchain.
 */

import { argon2idAsync } from '@noble/hashes/argon2.js';

const HASH_LEN_BYTES = 32;
const TIME_COST = 3;
const MEMORY_COST_KIB = 64 * 1024;
const PARALLELISM = 1;

export interface DeriveParams {
  passphrase: string | Uint8Array;
  salt: string | Uint8Array;
}

export async function deriveArgon2id(params: DeriveParams): Promise<Uint8Array> {
  const hash = await argon2idAsync(params.passphrase, params.salt, {
    t: TIME_COST,
    m: MEMORY_COST_KIB,
    p: PARALLELISM,
    dkLen: HASH_LEN_BYTES,
  });
  if (!(hash instanceof Uint8Array)) {
    throw new Error('argon2idAsync returned an unexpected payload');
  }
  if (hash.length !== HASH_LEN_BYTES) {
    throw new Error(`argon2idAsync hash length mismatch: expected ${HASH_LEN_BYTES}, received ${hash.length}`);
  }
  return hash;
}
