import { execFileSync } from 'node:child_process';
import { webcrypto } from 'node:crypto';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import { deriveNoiseTransportSecrets } from './noiseHandshake';
import { derivePreSharedKey } from './sharedKey';

if (!globalThis.crypto) {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  globalThis.crypto = webcrypto as unknown as Crypto;
}

const repoRoot = path.resolve(process.cwd(), '..', '..');
const rustInteropManifest = path.join(repoRoot, 'temp', 'crypto-interop', 'Cargo.toml');

type RustHandshakeOutput = {
  psk: string;
  send_key: string;
  recv_key: string;
  verification_code: string;
  challenge_key: string;
  challenge_context: string;
};

function runRustHandshake(payload: Record<string, unknown>): RustHandshakeOutput {
  const output = execFileSync(
    'cargo',
    ['run', '--quiet', '--manifest-path', rustInteropManifest, '--bin', 'noise_handshake'],
    {
      cwd: repoRoot,
      input: JSON.stringify(payload),
      encoding: 'utf8',
    },
  );
  return JSON.parse(output) as RustHandshakeOutput;
}

const bytesToBase64 = (value: Uint8Array): string => Buffer.from(value).toString('base64');

describe('noise handshake derivation interop', () => {
  const passphrase = 'InteropOtters!';
  const handshakeId = 'handshake-interop-001';
  const handshakeHash = Uint8Array.from([
    0x46, 0x85, 0xa3, 0x41, 0x5b, 0x95, 0xab, 0x90, 0x2c, 0xf1, 0x0c, 0xed, 0x77, 0xab, 0x89, 0x34,
    0x20, 0x98, 0xb2, 0xaa, 0x0f, 0x43, 0xcd, 0x12, 0x7e, 0xd1, 0x3d, 0x6c, 0x55, 0x44, 0x11, 0x05,
  ]);
  const handshakeHashBase64 = bytesToBase64(handshakeHash);

  it('matches Rust output for initiator perspective', async () => {
    const localPeerId = 'peer-offerer';
    const remotePeerId = 'peer-answerer';
    const psk = await derivePreSharedKey(passphrase, handshakeId);
    const secrets = await deriveNoiseTransportSecrets({
      handshakeHash,
      psk,
      handshakeId,
      localPeerId,
      remotePeerId,
    });

    const rust = runRustHandshake({
      passphrase,
      handshake_id: handshakeId,
      handshake_hash: handshakeHashBase64,
      local_peer_id: localPeerId,
      remote_peer_id: remotePeerId,
    });

    expect(rust.psk).toEqual(bytesToBase64(psk));
    expect(rust.send_key).toEqual(bytesToBase64(secrets.sendKey));
    expect(rust.recv_key).toEqual(bytesToBase64(secrets.recvKey));
    expect(rust.challenge_key).toEqual(bytesToBase64(secrets.challengeKey));
    expect(rust.challenge_context).toEqual(new TextDecoder().decode(secrets.challengeContext));
    expect(rust.verification_code).toEqual(secrets.verificationCode);
  });

  it('matches Rust output for responder perspective', async () => {
    const localPeerId = 'peer-answerer';
    const remotePeerId = 'peer-offerer';
    const psk = await derivePreSharedKey(passphrase, handshakeId);
    const secrets = await deriveNoiseTransportSecrets({
      handshakeHash,
      psk,
      handshakeId,
      localPeerId,
      remotePeerId,
    });

    const rust = runRustHandshake({
      passphrase,
      handshake_id: handshakeId,
      handshake_hash: handshakeHashBase64,
      local_peer_id: localPeerId,
      remote_peer_id: remotePeerId,
    });

    expect(rust.psk).toEqual(bytesToBase64(psk));
    expect(rust.send_key).toEqual(bytesToBase64(secrets.sendKey));
    expect(rust.recv_key).toEqual(bytesToBase64(secrets.recvKey));
    expect(rust.challenge_key).toEqual(bytesToBase64(secrets.challengeKey));
    expect(rust.challenge_context).toEqual(new TextDecoder().decode(secrets.challengeContext));
    expect(rust.verification_code).toEqual(secrets.verificationCode);
  });
});
