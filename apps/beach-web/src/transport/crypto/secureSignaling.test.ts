import { describe, expect, it } from 'vitest';

import { execFileSync } from 'node:child_process';
import { webcrypto } from 'node:crypto';
import path from 'node:path';

if (!globalThis.crypto) {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  globalThis.crypto = webcrypto as unknown as Crypto;
}

import { derivePreSharedKey } from './sharedKey';
import { openWithKey, sealWithKey } from './secureSignaling';

const repoRoot = path.resolve(process.cwd(), '..', '..');
const rustInteropManifest = path.join(repoRoot, 'temp', 'crypto-interop', 'Cargo.toml');

type RustInteropResponse = {
  psk: string;
  message_key: string;
  envelope?: { nonce: string; ciphertext: string };
  plaintext?: string;
};

function runRustInterop(payload: Record<string, unknown>): RustInteropResponse {
  const output = execFileSync(
    'cargo',
    ['run', '--quiet', '--manifest-path', rustInteropManifest, '--bin', 'crypto-interop-test'],
    {
      cwd: repoRoot,
      input: JSON.stringify(payload),
      encoding: 'utf8',
    },
  );
  return JSON.parse(output) as RustInteropResponse;
}

const base64ToBytes = (value: string): Uint8Array =>
  new Uint8Array(Buffer.from(value, 'base64'));
const bytesToBase64 = (value: Uint8Array): string => Buffer.from(value).toString('base64');

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

  it('interoperates with the Rust implementation', async () => {
    const passphrase = 'OttersPlayAtDawn';
    const handshakeId = 'handshake-1234';
    const associated = ['from-peer', 'to-peer', 'offer'] as const;
    const plaintext = 'v=0\r\no=- 46117359 2 IN IP4 127.0.0.1';
    const deterministicRustNonce = 'AAECAwQFBgcICQoL';
    const psk = await derivePreSharedKey(passphrase, handshakeId);

    const rustSeal = runRustInterop({
      passphrase,
      handshake_id: handshakeId,
      label: 'offer',
      associated,
      plaintext,
      nonce: deterministicRustNonce,
    });

    expect(rustSeal.envelope).toBeDefined();
    expect(rustSeal.psk).toEqual(bytesToBase64(psk));

    const rustEnvelope = {
      version: 1,
      nonce: rustSeal.envelope!.nonce,
      ciphertext: rustSeal.envelope!.ciphertext,
    };

    const opened = await openWithKey({
      psk: base64ToBytes(rustSeal.psk),
      handshakeId,
      label: 'offer',
      associatedData: [...associated],
      envelope: rustEnvelope,
    });

    expect(opened).toEqual(plaintext);

    const deterministicJsNonce = Uint8Array.from([
      0xde, 0xad, 0xbe, 0xef, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
    ]);
    const originalGetRandomValues = crypto.getRandomValues.bind(crypto);

    try {
      crypto.getRandomValues = ((buffer: Uint8Array) => {
        buffer.set(deterministicJsNonce);
        return buffer;
      }) as typeof crypto.getRandomValues;

      const jsEnvelope = await sealWithKey({
        psk,
        handshakeId,
        label: 'offer',
        associatedData: [...associated],
        plaintext,
      });

      expect(jsEnvelope.nonce).toEqual(bytesToBase64(deterministicJsNonce));

      const rustOpen = runRustInterop({
        passphrase,
        handshake_id: handshakeId,
        label: 'offer',
        associated,
        envelope: {
          nonce: jsEnvelope.nonce,
          ciphertext: jsEnvelope.ciphertext,
        },
      });

      expect(rustOpen.plaintext).toEqual(plaintext);
      expect(rustOpen.psk).toEqual(bytesToBase64(psk));
    } finally {
      crypto.getRandomValues = originalGetRandomValues;
    }
  });
});
