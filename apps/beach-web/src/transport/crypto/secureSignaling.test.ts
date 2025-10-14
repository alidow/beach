import { describe, expect, it } from 'vitest';

import { webcrypto } from 'node:crypto';

if (!globalThis.crypto) {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  globalThis.crypto = webcrypto as unknown as Crypto;
}

import { derivePreSharedKey } from './sharedKey';
import { openWithKey, sealWithKey } from './secureSignaling';

describe('secure signaling', () => {
  it('round-trips payloads using ChaCha20-Poly1305', async () => {
    const passphrase = 'OttersPlayAtDawn';
    const handshakeId = 'handshake-1234';
    const key = await derivePreSharedKey(passphrase, handshakeId);

    const envelope = await sealWithKey({
      psk: key,
      handshakeId,
      label: 'offer',
      associatedData: ['from-peer', 'to-peer', 'offer'],
      plaintext: 'v=0\r\no=- 46117359 2 IN IP4 127.0.0.1',
    });

    const plain = await openWithKey({
      psk: key,
      handshakeId,
      label: 'offer',
      associatedData: ['from-peer', 'to-peer', 'offer'],
      envelope,
    });

    expect(plain).toEqual('v=0\r\no=- 46117359 2 IN IP4 127.0.0.1');
  });
});
