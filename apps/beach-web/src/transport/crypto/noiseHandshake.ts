import createNoiseModule from 'noise-c.wasm';
import noiseWasmUrl from 'noise-c.wasm/src/noise-c.wasm?url';

import { derivePreSharedKey, hkdfExpand, toHex } from './sharedKey';

interface NoiseConstants {
  NOISE_ROLE_INITIATOR: number;
  NOISE_ROLE_RESPONDER: number;
  NOISE_ACTION_NONE: number;
  NOISE_ACTION_WRITE_MESSAGE: number;
  NOISE_ACTION_READ_MESSAGE: number;
  NOISE_ACTION_FAILED: number;
  NOISE_ACTION_SPLIT: number;
  NOISE_DH_CURVE25519: number;
  NOISE_DH_CURVE448: number;
}

interface NoiseCipherState {
  free(): void;
}

interface NoiseHandshakeState {
  Initialize(
    prologue: Uint8Array | null,
    s: Uint8Array | null,
    rs: Uint8Array | null,
    psk: Uint8Array | null,
  ): void;
  GetAction(): number;
  WriteMessage(payload: Uint8Array | null): Uint8Array;
  ReadMessage(
    message: Uint8Array,
    payloadNeeded?: boolean,
    fallbackSupported?: boolean,
  ): Uint8Array | null;
  Split(): [NoiseCipherState, NoiseCipherState];
  GetHandshakeHash(): Uint8Array;
  free(): void;
}

interface NoiseModule {
  constants: NoiseConstants;
  HandshakeState: new (protocolName: string, role: number) => NoiseHandshakeState;
  CreateKeyPair(curveId: number): [Uint8Array, Uint8Array];
}

const PROTOCOL_NAME = 'Noise_XX_25519_ChaChaPoly_BLAKE2s';
const PROLOGUE_PREFIX = 'beach:secure-handshake:v1';
const FIELD_SEPARATOR = 0x1f;
const TRANSPORT_DIRECTION_PREFIX = 'beach:secure-transport:direction:';
const TRANSPORT_VERIFY_PREFIX = 'beach:secure-transport:verify:';
const TRANSPORT_CHALLENGE_KEY_PREFIX = 'beach:secure-transport:challenge-key:';
const TRANSPORT_CHALLENGE_MAC_PREFIX = 'beach:secure-transport:challenge-mac:';
const CHALLENGE_FRAME_VERSION = 1;
const CHALLENGE_NONCE_LENGTH = 16;
const CHALLENGE_MAC_LENGTH = 32;
const CHALLENGE_FRAME_LENGTH = 1 + 1 + 6 + CHALLENGE_NONCE_LENGTH + CHALLENGE_MAC_LENGTH;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

type NoiseHandshake = NoiseHandshakeState;

export type BrowserHandshakeRole = 'initiator' | 'responder';

export interface BrowserHandshakeParams {
  role: BrowserHandshakeRole;
  handshakeId: string;
  localPeerId: string;
  remotePeerId: string;
  prologueContext: Uint8Array;
  passphrase?: string;
  preSharedKey?: Uint8Array;
  preSharedKeyPromise?: Promise<Uint8Array>;
}

export interface BrowserHandshakeResult {
  sendKey: Uint8Array;
  recvKey: Uint8Array;
  verificationCode: string;
}

export function buildPrologueContext(
  handshakeId: string,
  localPeerId: string,
  remotePeerId: string,
): Uint8Array {
  const peers = [localPeerId, remotePeerId].sort();
  const parts = [encoder.encode(handshakeId), encoder.encode(peers[0]), encoder.encode(peers[1])];
  const totalLength = parts.reduce((sum, item) => sum + item.length, 0) + (parts.length - 1);
  const output = new Uint8Array(totalLength);
  let offset = 0;
  parts.forEach((part, index) => {
    output.set(part, offset);
    offset += part.length;
    if (index < parts.length - 1) {
      output[offset++] = FIELD_SEPARATOR;
    }
  });
  return output;
}

export async function runBrowserHandshake(
  channel: RTCDataChannel,
  params: BrowserHandshakeParams,
): Promise<BrowserHandshakeResult> {
  channel.binaryType = 'arraybuffer';
  await waitForChannelOpen(channel);

  const noise = await loadNoise();
  const psk = await resolvePreSharedKey(params);
  const prologue = buildPrologue(params.prologueContext);
  const handshake = createHandshake(noise, params.role, prologue);
  const queue = new DataChannelQueue(channel);

  try {
    await driveHandshake(noise, handshake, queue, channel, params);
    const handshakeHash = new Uint8Array(handshake.GetHandshakeHash());
    const [sendCipher, recvCipher] = handshake.Split();
    // Immediately free the cipher states to avoid leaking WASM memory.
    try {
      sendCipher.free();
    } catch {}
    try {
      recvCipher.free();
    } catch {}
    const secrets = await deriveNoiseTransportSecrets({
      handshakeHash,
      psk,
      handshakeId: params.handshakeId,
      localPeerId: params.localPeerId,
      remotePeerId: params.remotePeerId,
    });
    await performVerificationExchange({
      channel,
      queue,
      role: params.role,
      handshakeId: params.handshakeId,
      localPeerId: params.localPeerId,
      remotePeerId: params.remotePeerId,
      verificationCode: secrets.verificationCode,
      challengeKey: secrets.challengeKey,
      challengeContext: secrets.challengeContext,
    });
    return {
      sendKey: secrets.sendKey,
      recvKey: secrets.recvKey,
      verificationCode: secrets.verificationCode,
    };
  } finally {
    queue.dispose();
    try {
      handshake.free();
    } catch {
      // Already freed by Split
    }
  }
}

async function resolvePreSharedKey(params: BrowserHandshakeParams): Promise<Uint8Array> {
  if (params.preSharedKey) {
    console.debug('[beach-web][noise] preSharedKey resolved', {
      handshakeId: params.handshakeId,
      source: 'provided',
      length: params.preSharedKey.length,
    });
    return params.preSharedKey;
  }
  if (params.preSharedKeyPromise) {
    console.debug('[beach-web][noise] waiting for preSharedKey promise', {
      handshakeId: params.handshakeId,
      source: 'promise',
    });
    const key = await params.preSharedKeyPromise;
    console.debug('[beach-web][noise] preSharedKey promise fulfilled', {
      handshakeId: params.handshakeId,
      source: 'promise',
      length: key.length,
    });
    return key;
  }
  if (!params.passphrase) {
    throw new Error('secure handshake requires passphrase or pre-shared key');
  }
  console.debug('[beach-web][noise] deriving preSharedKey from passphrase', {
    handshakeId: params.handshakeId,
    source: 'passphrase',
  });
  const key = await derivePreSharedKey(params.passphrase, params.handshakeId);
  console.debug('[beach-web][noise] derived preSharedKey from passphrase', {
    handshakeId: params.handshakeId,
    source: 'passphrase',
    length: key.length,
  });
  return key;
}

function buildPrologue(context: Uint8Array): Uint8Array {
  const prefix = encoder.encode(PROLOGUE_PREFIX);
  const prologue = new Uint8Array(prefix.length + 1 + context.length);
  prologue.set(prefix, 0);
  prologue[prefix.length] = FIELD_SEPARATOR;
  prologue.set(context, prefix.length + 1);
  return prologue;
}

function createHandshake(
  noise: NoiseModule,
  role: BrowserHandshakeRole,
  prologue: Uint8Array,
): NoiseHandshake {
  const roleConstant =
    role === 'initiator'
      ? noise.constants.NOISE_ROLE_INITIATOR
      : noise.constants.NOISE_ROLE_RESPONDER;
  const exportKeys = Object.keys(noise as unknown as Record<string, unknown>);
  console.debug('[beach-web][noise] createHandshake', {
    protocol: PROTOCOL_NAME,
    role,
    roleConstant,
    exportKeys,
    constants: noise.constants,
    handshakeStateType: typeof (noise as unknown as Record<string, unknown>).HandshakeState,
    patterns: (noise as unknown as Record<string, unknown>).HandshakePatterns ?? null,
  });
  let handshake: NoiseHandshake;
  try {
    handshake = new noise.HandshakeState(PROTOCOL_NAME, roleConstant);
  } catch (error) {
    console.error('[beach-web][noise] HandshakeState construction failed', {
      protocol: PROTOCOL_NAME,
      role,
      roleConstant,
      exportKeys,
      constants: noise.constants,
      patterns: (noise as unknown as Record<string, unknown>).HandshakePatterns ?? null,
      error,
    });
    throw error;
  }
  console.debug('[beach-web][noise] HandshakeState constructed', {
    protocol: PROTOCOL_NAME,
    role,
    roleConstant,
  });
  const keypair = noise.CreateKeyPair(noise.constants.NOISE_DH_CURVE25519);
  console.debug('[beach-web][noise] CreateKeyPair result', {
    keypair,
    isArray: Array.isArray(keypair),
    length: keypair?.length,
    privateKeyType: typeof keypair?.[0],
    privateKeyLength: keypair?.[0]?.length,
    publicKeyType: typeof keypair?.[1],
    publicKeyLength: keypair?.[1]?.length,
  });
  const [privateKey] = keypair;
  console.debug('[beach-web][noise] About to Initialize', {
    privateKeyType: typeof privateKey,
    privateKeyLength: privateKey?.length,
    privateKeyIsNull: privateKey === null,
    privateKeyIsUndefined: privateKey === undefined,
  });
  try {
    handshake.Initialize(prologue, privateKey, null, null);
  } finally {
    privateKey.fill(0);
  }
  return handshake;
}

async function driveHandshake(
  noise: NoiseModule,
  handshake: NoiseHandshake,
  queue: DataChannelQueue,
  channel: RTCDataChannel,
  params: BrowserHandshakeParams,
): Promise<void> {
  let writeCount = 0;
  let readCount = 0;
  while (true) {
    const action = handshake.GetAction();
    switch (action) {
      case noise.constants.NOISE_ACTION_WRITE_MESSAGE: {
        const message = handshake.WriteMessage(null);
        console.debug('[beach-web][noise] handshake_write', {
          handshakeId: params.handshakeId,
          role: params.role,
          index: writeCount,
          bytes: message.length,
        });
        writeCount += 1;
        channel.send(toArrayBuffer(message));
        break;
      }
      case noise.constants.NOISE_ACTION_READ_MESSAGE: {
        const incoming = await queue.next();
        console.debug('[beach-web][noise] handshake_read', {
          handshakeId: params.handshakeId,
          role: params.role,
          index: readCount,
          bytes: incoming.length,
        });
        readCount += 1;
        handshake.ReadMessage(incoming, false, false);
        break;
      }
      case noise.constants.NOISE_ACTION_SPLIT:
        console.debug('[beach-web][noise] handshake_split', {
          handshakeId: params.handshakeId,
          role: params.role,
          outboundMessages: writeCount,
          inboundMessages: readCount,
        });
        return;
      case noise.constants.NOISE_ACTION_NONE:
        await queue.idle();
        break;
      case noise.constants.NOISE_ACTION_FAILED:
      default:
        throw new Error('noise handshake failed');
    }
  }
}

export interface NoiseTransportSecrets {
  sendKey: Uint8Array;
  recvKey: Uint8Array;
  verificationCode: string;
  challengeKey: Uint8Array;
  challengeContext: Uint8Array;
}

interface TransportSecretsInput {
  handshakeHash: Uint8Array;
  psk: Uint8Array;
  handshakeId: string;
  localPeerId: string;
  remotePeerId: string;
}

export async function deriveNoiseTransportSecrets(
  input: TransportSecretsInput,
): Promise<NoiseTransportSecrets> {
  const directionOut = encoder.encode(
    `${TRANSPORT_DIRECTION_PREFIX}${input.localPeerId}->${input.remotePeerId}`,
  );
  const directionIn = encoder.encode(
    `${TRANSPORT_DIRECTION_PREFIX}${input.remotePeerId}->${input.localPeerId}`,
  );

  const peers = [input.localPeerId, input.remotePeerId].sort();
  const verifyLabel = encoder.encode(`${TRANSPORT_VERIFY_PREFIX}${peers[0]}|${peers[1]}`);

  const sendMaterial = await hkdfExpand(input.handshakeHash, input.psk, directionOut, 32);
  const recvMaterial = await hkdfExpand(input.handshakeHash, input.psk, directionIn, 32);
  const verifyBytes = await hkdfExpand(input.handshakeHash, input.psk, verifyLabel, 4);
  const code =
    (verifyBytes[0]! |
      (verifyBytes[1]! << 8) |
      (verifyBytes[2]! << 16) |
      (verifyBytes[3]! << 24)) >>>
    0;
  const verificationCode = `${code % 1_000_000}`.padStart(6, '0');

  const challengeInfo = encoder.encode(
    `${TRANSPORT_CHALLENGE_KEY_PREFIX}${input.handshakeId}|${peers[0]}|${peers[1]}`,
  );
  const challengeKey = await hkdfExpand(input.handshakeHash, input.psk, challengeInfo, 32);
  const challengeContext = encoder.encode(
    `${TRANSPORT_CHALLENGE_MAC_PREFIX}${input.handshakeId}|${peers[0]}|${peers[1]}`,
  );

  return {
    sendKey: sendMaterial,
    recvKey: recvMaterial,
    verificationCode,
    challengeKey,
    challengeContext,
  };
}

interface VerificationExchangeParams {
  channel: RTCDataChannel;
  queue: DataChannelQueue;
  role: BrowserHandshakeRole;
  handshakeId: string;
  localPeerId: string;
  remotePeerId: string;
  verificationCode: string;
  challengeKey: Uint8Array;
  challengeContext: Uint8Array;
}

async function performVerificationExchange(params: VerificationExchangeParams): Promise<void> {
  const codeBytes = encoder.encode(params.verificationCode);
  if (codeBytes.length !== 6) {
    throw new Error('verification code must be 6 characters');
  }

  const roleByte = params.role === 'initiator' ? 0 : 1;
  const expectedRemoteRole = params.role === 'initiator' ? 1 : 0;
  const baseDetails = {
    handshakeId: params.handshakeId,
    localPeerId: params.localPeerId,
    remotePeerId: params.remotePeerId,
    role: params.role,
  };

  const nonce = new Uint8Array(CHALLENGE_NONCE_LENGTH);
  crypto.getRandomValues(nonce);
  console.debug('[beach-web][noise] challenge_prepare', {
    ...baseDetails,
    roleByte,
    code: params.verificationCode,
    nonce: toHex(nonce),
  });

  try {
    const outboundFrame = await buildChallengeFrame({
      roleByte,
      codeBytes,
      nonce,
      challengeKey: params.challengeKey,
      challengeContext: params.challengeContext,
    });
    const outboundMac = outboundFrame.slice(2 + 6 + CHALLENGE_NONCE_LENGTH);
    console.debug('[beach-web][noise] challenge_mac_computed', {
      ...baseDetails,
      mac: toHex(outboundMac),
    });
    params.channel.send(toArrayBuffer(outboundFrame));
    console.debug('[beach-web][noise] challenge_sent', baseDetails);

    const remotePayload = await params.queue.next();
    console.debug('[beach-web][noise] challenge_received_raw', {
      ...baseDetails,
      bytes: remotePayload.length,
    });
    const remoteFrame = parseChallengeFrame(remotePayload);
    console.debug('[beach-web][noise] challenge_parsed', {
      ...baseDetails,
      remoteRole: remoteFrame.role,
      remoteCodeHex: toHex(remoteFrame.codeBytes),
      remoteNonce: toHex(remoteFrame.nonce),
      remoteMac: toHex(remoteFrame.mac),
    });

    if (remoteFrame.version !== CHALLENGE_FRAME_VERSION) {
      verificationFailure(baseDetails, 'secure handshake verification failed', {
        case: 'unexpected-version',
        observedVersion: remoteFrame.version,
      });
    }

    if (remoteFrame.role !== expectedRemoteRole) {
      verificationFailure(baseDetails, 'secure handshake verification failed', {
        case: 'unexpected-role',
        observedRole: remoteFrame.role,
        expectedRole: expectedRemoteRole,
      });
    }

    const expectedMac = await computeChallengeMac({
      roleByte: remoteFrame.role,
      codeBytes: remoteFrame.codeBytes,
      nonce: remoteFrame.nonce,
      challengeKey: params.challengeKey,
      challengeContext: params.challengeContext,
    });

    if (!timingSafeEqual(remoteFrame.mac, expectedMac)) {
      verificationFailure(baseDetails, 'secure handshake verification failed', {
        case: 'mac-mismatch',
        expectedMac: toHex(expectedMac),
        observedMac: toHex(remoteFrame.mac),
      });
    }
    console.debug('[beach-web][noise] challenge_mac_verified', {
      ...baseDetails,
      expectedMac: toHex(expectedMac),
    });

    const remoteCode = decoder.decode(remoteFrame.codeBytes);
    if (remoteCode !== params.verificationCode) {
      verificationFailure(baseDetails, 'secure handshake verification failed', {
        case: 'verification-code-mismatch',
        localCode: params.verificationCode,
        remoteCode,
      });
    }
    console.debug('[beach-web][noise] challenge_codes_match', {
      ...baseDetails,
      remoteCode,
      localCode: params.verificationCode,
    });
  } catch (error) {
    const details =
      error instanceof Error && (error as any).verificationDetails
        ? (error as any).verificationDetails
        : {
            ...baseDetails,
            case: 'unexpected-error',
            reason: error instanceof Error ? error.message : String(error),
          };
    console.warn('[beach-web][noise] post-handshake verification failed', details);
    try {
      params.channel.close();
    } catch {}
    throw error instanceof Error ? error : new Error(String(error));
  }
}

interface ChallengeFrame {
  version: number;
  role: number;
  codeBytes: Uint8Array;
  nonce: Uint8Array;
  mac: Uint8Array;
}

interface ChallengeFrameInput {
  roleByte: number;
  codeBytes: Uint8Array;
  nonce: Uint8Array;
  challengeKey: Uint8Array;
  challengeContext: Uint8Array;
}

async function buildChallengeFrame(input: ChallengeFrameInput): Promise<Uint8Array> {
  if (input.codeBytes.length !== 6) {
    throw new Error('challenge code must be 6 bytes');
  }
  if (input.nonce.length !== CHALLENGE_NONCE_LENGTH) {
    throw new Error(`challenge nonce must be ${CHALLENGE_NONCE_LENGTH} bytes`);
  }

  const frame = new Uint8Array(CHALLENGE_FRAME_LENGTH);
  frame[0] = CHALLENGE_FRAME_VERSION;
  frame[1] = input.roleByte;
  frame.set(input.codeBytes, 2);
  frame.set(input.nonce, 2 + 6);

  const mac = await computeChallengeMac({
    roleByte: input.roleByte,
    codeBytes: input.codeBytes,
    nonce: input.nonce,
    challengeKey: input.challengeKey,
    challengeContext: input.challengeContext,
  });

  frame.set(mac, 2 + 6 + CHALLENGE_NONCE_LENGTH);
  return frame;
}

function parseChallengeFrame(payload: Uint8Array): ChallengeFrame {
  if (payload.length !== CHALLENGE_FRAME_LENGTH) {
    throw new Error(`challenge frame length mismatch (${payload.length})`);
  }
  const codeStart = 2;
  const codeEnd = codeStart + 6;
  const nonceEnd = codeEnd + CHALLENGE_NONCE_LENGTH;
  return {
    version: payload[0]!,
    role: payload[1]!,
    codeBytes: payload.slice(codeStart, codeEnd),
    nonce: payload.slice(codeEnd, nonceEnd),
    mac: payload.slice(nonceEnd),
  };
}

interface ChallengeMacInput {
  roleByte: number;
  codeBytes: Uint8Array;
  nonce: Uint8Array;
  challengeKey: Uint8Array;
  challengeContext: Uint8Array;
}

async function computeChallengeMac(input: ChallengeMacInput): Promise<Uint8Array> {
  const macInput = concatBytes(
    input.challengeContext,
    new Uint8Array([input.roleByte & 0xff]),
    input.codeBytes,
    input.nonce,
  );
  return await computeHmacSha256(input.challengeKey, macInput);
}

async function computeHmacSha256(key: Uint8Array, data: Uint8Array): Promise<Uint8Array> {
  const cryptoKey = await crypto.subtle.importKey(
    'raw',
    toArrayBuffer(key),
    {
      name: 'HMAC',
      hash: 'SHA-256',
    },
    false,
    ['sign'],
  );
  const mac = await crypto.subtle.sign('HMAC', cryptoKey, toArrayBuffer(data));
  return new Uint8Array(mac);
}

function concatBytes(...chunks: Uint8Array[]): Uint8Array {
  const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const output = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    output.set(chunk, offset);
    offset += chunk.length;
  }
  return output;
}

function timingSafeEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) {
    return false;
  }
  let diff = 0;
  for (let i = 0; i < a.length; i += 1) {
    diff |= a[i]! ^ b[i]!;
  }
  return diff === 0;
}

function verificationFailure(
  base: Record<string, unknown>,
  reason: string,
  extra: Record<string, unknown>,
): never {
  const error = new Error(reason);
  Object.assign(error, {
    verificationDetails: {
      ...base,
      ...extra,
    },
  });
  throw error;
}

async function waitForChannelOpen(channel: RTCDataChannel): Promise<void> {
  if (channel.readyState === 'open') {
    return;
  }
  if (channel.readyState === 'closing' || channel.readyState === 'closed') {
    throw new Error('handshake channel closed before opening');
  }
  await new Promise<void>((resolve, reject) => {
    const handleOpen = () => {
      cleanup();
      resolve();
    };
    const handleClose = () => {
      cleanup();
      reject(new Error('handshake channel closed before opening'));
    };
    const handleError = (event: Event) => {
      cleanup();
      const error = (event as any).error ?? new Error('handshake channel error');
      reject(error instanceof Error ? error : new Error(String(error)));
    };
    const cleanup = () => {
      channel.removeEventListener('open', handleOpen);
      channel.removeEventListener('close', handleClose);
      channel.removeEventListener('error', handleError);
    };
    channel.addEventListener('open', handleOpen, { once: true });
    channel.addEventListener('close', handleClose, { once: true });
    channel.addEventListener('error', handleError, { once: true });
  });
}

class DataChannelQueue {
  private readonly channel: RTCDataChannel;
  private readonly messages: Uint8Array[] = [];
  private readonly resolvers: Array<(value: Uint8Array) => void> = [];
  private readonly rejecters: Array<(reason: unknown) => void> = [];
  private settledError: unknown = null;
  private idleResolvers: Array<() => void> = [];

  private readonly onMessage = (event: MessageEvent) => {
    try {
      const payload = normaliseData(event.data);
      console.debug('[beach-web][noise] queue_message', {
        bytes: payload.length,
        bufferedResolvers: this.resolvers.length,
      });
      if (this.resolvers.length > 0) {
        const resolve = this.resolvers.shift()!;
        resolve(payload);
      } else {
        this.messages.push(payload);
      }
      this.resolveIdle();
    } catch (error) {
      this.fail(error);
    }
  };

  private readonly onClose = () => {
    this.fail(new Error('handshake channel closed'));
  };

  private readonly onError = (event: Event) => {
    const error = (event as any).error ?? new Error('handshake channel error');
    this.fail(error);
  };

  constructor(channel: RTCDataChannel) {
    this.channel = channel;
    channel.addEventListener('message', this.onMessage);
    channel.addEventListener('close', this.onClose);
    channel.addEventListener('error', this.onError);
  }

  async next(): Promise<Uint8Array> {
    if (this.messages.length > 0) {
      return this.messages.shift()!;
    }
    if (this.settledError) {
      throw this.settledError instanceof Error
        ? this.settledError
        : new Error(String(this.settledError));
    }
    return await new Promise<Uint8Array>((resolve, reject) => {
      this.resolvers.push(resolve);
      this.rejecters.push(reject);
    });
  }

  async idle(): Promise<void> {
    if (this.messages.length > 0) {
      return;
    }
    if (this.settledError) {
      return;
    }
    await new Promise<void>((resolve) => {
      this.idleResolvers.push(resolve);
    });
  }

  dispose(): void {
    this.channel.removeEventListener('message', this.onMessage);
    this.channel.removeEventListener('close', this.onClose);
    this.channel.removeEventListener('error', this.onError);
    this.fail(new Error('handshake queue disposed'));
  }

  private fail(error: unknown): void {
    if (this.settledError) {
      return;
    }
    this.settledError = error;
    console.debug('[beach-web][noise] queue_fail', {
      reason: error instanceof Error ? error.message : String(error),
    });
    while (this.rejecters.length > 0) {
      const reject = this.rejecters.shift()!;
      reject(error);
    }
    this.resolvers.length = 0;
    this.resolveIdle();
  }

  private resolveIdle(): void {
    while (this.idleResolvers.length > 0) {
      const resolve = this.idleResolvers.shift()!;
      resolve();
    }
  }
}

function normaliseData(data: unknown): Uint8Array {
  if (data instanceof ArrayBuffer) {
    return new Uint8Array(data);
  }
  if (ArrayBuffer.isView(data)) {
    const view = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    return view.slice();
  }
  throw new Error('expected binary RTCDataChannel payload');
}

let noiseModulePromise: Promise<NoiseModule> | null = null;

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

async function loadNoise(): Promise<NoiseModule> {
  if (!noiseModulePromise) {
    noiseModulePromise = (async () => {
      const wasmBinary = await resolveWasmBinary();
      console.debug('[beach-web][noise] resolveWasmBinary result', {
        providedBinary: Boolean(wasmBinary),
        environment: typeof window === 'undefined' ? 'node' : 'browser',
      });
      return await new Promise<NoiseModule>((resolve, reject) => {
        try {
          const options: { locateFile: (path: string) => string; wasmBinary?: Uint8Array } = {
            locateFile: (path: string) => (path.endsWith('.wasm') ? noiseWasmUrl : path),
          };
          if (wasmBinary) {
            options.wasmBinary = wasmBinary;
            console.debug('[beach-web][noise] supplying wasmBinary bytes', {
              byteLength: wasmBinary.byteLength,
            });
          }
          createNoiseModule(options, (module) => {
            try {
              const typed = module as NoiseModule;
              const exports = Object.keys(module as unknown as Record<string, unknown>);
              console.debug('[beach-web][noise] module resolved', {
                exportKeys: exports,
                hasHandshakeState: typeof (module as unknown as Record<string, unknown>)
                  .HandshakeState,
                constants: typed.constants,
                patterns: (module as unknown as Record<string, unknown>).HandshakePatterns ?? null,
              });
              resolve(typed);
            } catch (error) {
              reject(error);
            }
          });
        } catch (error) {
          reject(error);
        }
      });
    })();
  }
  return await noiseModulePromise;
}

async function resolveWasmBinary(): Promise<Uint8Array | undefined> {
  if (typeof window !== 'undefined' && typeof window.document !== 'undefined') {
    return undefined;
  }
  try {
    const [{ readFileSync }, { fileURLToPath }] = await Promise.all([
      import('node:fs'),
      import('node:url'),
    ]);
    const resolvedUrl =
      typeof (import.meta as any).resolve === 'function'
        ? await (import.meta as any).resolve('noise-c.wasm/src/noise-c.wasm')
        : new URL(
            '../../../../../node_modules/noise-c.wasm/src/noise-c.wasm',
            import.meta.url,
          ).toString();
    const fileUrl = new URL(resolvedUrl);
    const filePath = fileURLToPath(fileUrl);
    console.debug('[beach-web][noise] resolved wasm file path', { filePath });
    const buffer = readFileSync(filePath);
    return new Uint8Array(
      buffer.buffer.slice(buffer.byteOffset, buffer.byteOffset + buffer.byteLength),
    );
  } catch (error) {
    console.error('[beach-web][noise] resolveWasmBinary failed', error);
    return undefined;
  }
}
