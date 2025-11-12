import { chacha20poly1305Decrypt, chacha20poly1305Encrypt } from './chachaPoly';
import { derivePreSharedKey, hkdfExpand } from './sharedKey';

export interface SealedEnvelope {
  version: number;
  nonce: string;
  ciphertext: string;
}

export type SignalingLabel = 'offer' | 'answer' | 'ice';

const SIGNALING_VERSION = 1;
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const HKDF_INFO_AEAD = encoder.encode('beach:secure-signaling:aead:v1');
const LABEL_BYTES: Record<SignalingLabel, Uint8Array> = {
  offer: encoder.encode('offer'),
  answer: encoder.encode('answer'),
  ice: encoder.encode('ice'),
};
const INSECURE_OVERRIDE_TOKEN = 'I_KNOW_THIS_IS_UNSAFE';

export function secureSignalingEnabled(): boolean {
  const metaEnv = (import.meta as unknown as { env?: Record<string, string | undefined> }).env;
  const override = metaEnv?.VITE_ALLOW_PLAINTEXT ?? '';
  return override.trim() !== INSECURE_OVERRIDE_TOKEN;
}

export async function sealSignalingMessage(options: {
  passphrase: string;
  handshakeId: string;
  label: SignalingLabel;
  associatedData: string[];
  plaintext: string;
}): Promise<SealedEnvelope> {
  const { passphrase, handshakeId, label, associatedData, plaintext } = options;
  const psk = await derivePreSharedKey(passphrase, handshakeId);
  return await sealWithKey({
    psk,
    handshakeId,
    label,
    associatedData,
    plaintext,
  });
}

export async function sealWithKey(options: {
  psk: Uint8Array;
  handshakeId: string;
  label: SignalingLabel;
  associatedData: string[];
  plaintext: string;
}): Promise<SealedEnvelope> {
  const { psk, handshakeId, label, associatedData, plaintext } = options;
  const key = await deriveMessageKey(psk, handshakeId, label);
  const aad = buildAssociatedData(handshakeId, label, associatedData);
  const nonce = randomNonce();
  const ciphertext = chacha20poly1305Encrypt(
    key,
    nonce,
    aad,
    encoder.encode(plaintext),
  );
  return {
    version: SIGNALING_VERSION,
    nonce: toBase64(nonce),
    ciphertext: toBase64(ciphertext),
  };
}

export async function openSignalingMessage(options: {
  passphrase: string;
  handshakeId: string;
  label: SignalingLabel;
  associatedData: string[];
  envelope: SealedEnvelope;
}): Promise<string> {
  const { passphrase, handshakeId, label, associatedData, envelope } = options;
  const psk = await derivePreSharedKey(passphrase, handshakeId);
  return await openWithKey({
    psk,
    handshakeId,
    label,
    associatedData,
    envelope,
  });
}

export async function openWithKey(options: {
  psk: Uint8Array;
  handshakeId: string;
  label: SignalingLabel;
  associatedData: string[];
  envelope: SealedEnvelope;
}): Promise<string> {
  const { psk, handshakeId, label, associatedData, envelope } = options;
  if (envelope.version !== SIGNALING_VERSION) {
    throw new Error(`unsupported sealed signaling version ${envelope.version}`);
  }
  const key = await deriveMessageKey(psk, handshakeId, label);
  const aad = buildAssociatedData(handshakeId, label, associatedData);
  const nonce = fromBase64(envelope.nonce);
  const ciphertext = fromBase64(envelope.ciphertext);
  const plaintext = chacha20poly1305Decrypt(key, nonce, aad, ciphertext);
  return decoder.decode(plaintext);
}

async function deriveMessageKey(
  psk: Uint8Array,
  handshakeId: string,
  label: SignalingLabel,
): Promise<Uint8Array> {
  const salt = encoder.encode(handshakeId);
  const info = concatBytes(HKDF_INFO_AEAD, LABEL_BYTES[label]);
  return await hkdfExpand(psk, salt, info, 32);
}

function buildAssociatedData(
  handshakeId: string,
  label: SignalingLabel,
  associated: string[],
): Uint8Array {
  const separator = 0x1f;
  const parts: number[] = [];
  const pushString = (value: string) => {
    const bytes = encoder.encode(value);
    for (const byte of bytes) {
      parts.push(byte);
    }
  };

  pushString(handshakeId);
  parts.push(separator);
  pushString(label);
  for (const item of associated) {
    parts.push(separator);
    pushString(item);
  }
  return new Uint8Array(parts);
}

function concatBytes(...arrays: Uint8Array[]): Uint8Array {
  const total = arrays.reduce((sum, arr) => sum + arr.length, 0);
  const output = new Uint8Array(total);
  let offset = 0;
  for (const arr of arrays) {
    output.set(arr, offset);
    offset += arr.length;
  }
  return output;
}

function randomNonce(): Uint8Array {
  const nonce = new Uint8Array(12);
  crypto.getRandomValues(nonce);
  return nonce;
}

function toBase64(bytes: Uint8Array): string {
  if (typeof Buffer !== 'undefined') {
    return Buffer.from(bytes).toString('base64');
  }
  let binary = '';
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}

function fromBase64(input: string): Uint8Array {
  if (typeof Buffer !== 'undefined') {
    return new Uint8Array(Buffer.from(input, 'base64'));
  }
  const binary = atob(input);
  const result = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    result[i] = binary.charCodeAt(i);
  }
  return result;
}
