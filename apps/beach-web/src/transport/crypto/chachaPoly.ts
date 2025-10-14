/**
 * Minimal ChaCha20-Poly1305 implementation in TypeScript.
 *
 * The code is adapted from the RFC 8439 specification and mirrors the layout used by
 * Rust's `chacha20poly1305` crate: ciphertext is returned with the 16-byte tag appended.
 *
 * This module intentionally keeps the implementation self-contained to avoid adding new
 * dependencies to the web bundle.
 */

const SIGMA = new Uint32Array([
  0x61707865, 0x3320646e, 0x79622d32, 0x6b206574,
]);

const BLOCK_SIZE = 64;
const TAG_SIZE = 16;
const POLY1305_P = (1n << 130n) - 5n;

function rotateLeft(value: number, shift: number): number {
  return ((value << shift) | (value >>> (32 - shift))) >>> 0;
}

function quarterRound(state: Uint32Array, a: number, b: number, c: number, d: number): void {
  state[a] = (state[a] + state[b]) >>> 0;
  state[d] = rotateLeft(state[d] ^ state[a], 16);

  state[c] = (state[c] + state[d]) >>> 0;
  state[b] = rotateLeft(state[b] ^ state[c], 12);

  state[a] = (state[a] + state[b]) >>> 0;
  state[d] = rotateLeft(state[d] ^ state[a], 8);

  state[c] = (state[c] + state[d]) >>> 0;
  state[b] = rotateLeft(state[b] ^ state[c], 7);
}

function chacha20Block(key: Uint8Array, counter: number, nonce: Uint8Array): Uint8Array {
  const state = new Uint32Array(16);
  state.set(SIGMA, 0);
  const keyView = new DataView(key.buffer, key.byteOffset, key.byteLength);
  for (let i = 0; i < 8; i++) {
    state[4 + i] = keyView.getUint32(i * 4, true);
  }
  state[12] = counter;
  const nonceView = new DataView(nonce.buffer, nonce.byteOffset, nonce.byteLength);
  state[13] = nonceView.getUint32(0, true);
  state[14] = nonceView.getUint32(4, true);
  state[15] = nonceView.getUint32(8, true);

  const working = new Uint32Array(state);
  for (let i = 0; i < 10; i++) {
    quarterRound(working, 0, 4, 8, 12);
    quarterRound(working, 1, 5, 9, 13);
    quarterRound(working, 2, 6, 10, 14);
    quarterRound(working, 3, 7, 11, 15);
    quarterRound(working, 0, 5, 10, 15);
    quarterRound(working, 1, 6, 11, 12);
    quarterRound(working, 2, 7, 8, 13);
    quarterRound(working, 3, 4, 9, 14);
  }

  for (let i = 0; i < 16; i++) {
    working[i] = (working[i] + state[i]) >>> 0;
  }

  const output = new Uint8Array(64);
  const outView = new DataView(output.buffer);
  for (let i = 0; i < 16; i++) {
    outView.setUint32(i * 4, working[i], true);
  }
  return output;
}

function chacha20XorStream(
  key: Uint8Array,
  nonce: Uint8Array,
  plaintext: Uint8Array,
  counter: number,
): Uint8Array {
  const ciphertext = new Uint8Array(plaintext.length);
  const blocks = Math.ceil(plaintext.length / BLOCK_SIZE);
  for (let block = 0; block < blocks; block++) {
    const keystream = chacha20Block(key, counter + block, nonce);
    const offset = block * BLOCK_SIZE;
    const length = Math.min(BLOCK_SIZE, plaintext.length - offset);
    for (let i = 0; i < length; i++) {
      ciphertext[offset + i] = plaintext[offset + i] ^ keystream[i];
    }
  }
  return ciphertext;
}

function readBigIntLE(bytes: Uint8Array): bigint {
  let value = 0n;
  for (let i = bytes.length - 1; i >= 0; i--) {
    value = (value << 8n) + BigInt(bytes[i]);
  }
  return value;
}

function clampR(r: bigint): bigint {
  const mask = BigInt('0x0ffffffc0ffffffc0ffffffc0fffffff');
  return r & mask;
}

function bigIntToBytesLE(value: bigint, length: number): Uint8Array {
  const result = new Uint8Array(length);
  for (let i = 0; i < length; i++) {
    result[i] = Number((value >> BigInt(8 * i)) & 0xffn);
  }
  return result;
}

function poly1305Mac(key: Uint8Array, aad: Uint8Array, ciphertext: Uint8Array): Uint8Array {
  const r = clampR(readBigIntLE(key.subarray(0, 16)));
  const s = readBigIntLE(key.subarray(16, 32));

  let acc = 0n;

  const process = (data: Uint8Array) => {
    for (let offset = 0; offset < data.length; offset += 16) {
      const block = data.subarray(offset, Math.min(offset + 16, data.length));
      let n = 0n;
      for (let i = 0; i < block.length; i++) {
        n += BigInt(block[i]) << BigInt(8 * i);
      }
      n += 1n << BigInt(8 * block.length);
      acc = (acc + n) % POLY1305_P;
      acc = (acc * r) % POLY1305_P;
    }
  };

  process(aad);
  process(ciphertext);

  const lengthBlock = new Uint8Array(16);
  const lengthView = new DataView(lengthBlock.buffer);
  lengthView.setBigUint64(0, BigInt(aad.length), true);
  lengthView.setBigUint64(8, BigInt(ciphertext.length), true);
  process(lengthBlock);

  const tag = (acc + s) % (1n << 128n);
  return bigIntToBytesLE(tag, 16);
}

function poly1305KeyGen(key: Uint8Array, nonce: Uint8Array): Uint8Array {
  const block = chacha20Block(key, 0, nonce);
  return block.subarray(0, 32);
}

export function chacha20poly1305Encrypt(
  key: Uint8Array,
  nonce: Uint8Array,
  aad: Uint8Array,
  plaintext: Uint8Array,
): Uint8Array {
  const polyKey = poly1305KeyGen(key, nonce);
  const ciphertext = chacha20XorStream(key, nonce, plaintext, 1);
  const tag = poly1305Mac(polyKey, aad, ciphertext);
  const result = new Uint8Array(ciphertext.length + TAG_SIZE);
  result.set(ciphertext, 0);
  result.set(tag, ciphertext.length);
  return result;
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
  const ciphertext = ciphertextAndTag.subarray(0, ciphertextAndTag.length - TAG_SIZE);
  const tag = ciphertextAndTag.subarray(ciphertext.length);
  const polyKey = poly1305KeyGen(key, nonce);
  const expectedTag = poly1305Mac(polyKey, aad, ciphertext);
  if (!constantTimeEqual(tag, expectedTag)) {
    throw new Error('authentication tag mismatch');
  }
  return chacha20XorStream(key, nonce, ciphertext, 1);
}

function constantTimeEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) {
    return false;
  }
  let diff = 0;
  for (let i = 0; i < a.length; i++) {
    diff |= a[i] ^ b[i];
  }
  return diff === 0;
}
