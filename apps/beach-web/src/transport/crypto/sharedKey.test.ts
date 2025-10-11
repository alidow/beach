import { webcrypto } from 'node:crypto';
import { describe, expect, it } from 'vitest';

if (!globalThis.crypto) {
  // Vitest under Node provides the Web Crypto API via node:crypto.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  globalThis.crypto = webcrypto as unknown as Crypto;
}

import { derivePreSharedKey, hkdfExpand, toHex } from './sharedKey';

const encoder = new TextEncoder();

describe('shared key derivation', () => {
  it('derives deterministic pre-shared keys', async () => {
    const passphrase = 'Otters-Play-At-Dawn';
    const handshakeId = 'handshake-12345';

    const keyA = await derivePreSharedKey(passphrase, handshakeId);
    const keyB = await derivePreSharedKey(passphrase, handshakeId);

    expect(toHex(keyA)).toEqual(toHex(keyB));
    expect(keyA).toHaveLength(32);
  });

  it('produces distinct HKDF outputs for different info labels', async () => {
    const ikm = encoder.encode('shared-secret');
    const salt = encoder.encode('handshake-v1');

    const sendKey = await hkdfExpand(ikm, salt, encoder.encode('send'), 32);
    const recvKey = await hkdfExpand(ikm, salt, encoder.encode('recv'), 32);

    expect(toHex(sendKey)).not.toEqual(toHex(recvKey));
    expect(sendKey).toHaveLength(32);
    expect(recvKey).toHaveLength(32);
  });
});
