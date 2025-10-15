import createNoiseModule from 'noise-c.wasm';
import noiseWasmUrl from 'noise-c.wasm/src/noise-c.wasm?url';

import { derivePreSharedKey, hkdfExpand } from './sharedKey';

interface NoiseConstants {
  NOISE_ROLE_INITIATOR: number;
  NOISE_ROLE_RESPONDER: number;
  NOISE_ACTION_NONE: number;
  NOISE_ACTION_WRITE_MESSAGE: number;
  NOISE_ACTION_READ_MESSAGE: number;
  NOISE_ACTION_FAILED: number;
  NOISE_ACTION_SPLIT: number;
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
  ReadMessage(message: Uint8Array, payloadNeeded?: boolean, fallbackSupported?: boolean): Uint8Array | null;
  Split(): [NoiseCipherState, NoiseCipherState];
  GetHandshakeHash(): Uint8Array;
  free(): void;
}

interface NoiseModule {
  constants: NoiseConstants;
  HandshakeState: new (protocolName: string, role: number) => NoiseHandshakeState;
}

const PROTOCOL_NAME = 'Noise_XXpsk2_25519_ChaChaPoly_BLAKE2s';
const PROLOGUE_PREFIX = 'beach:secure-handshake:v1';
const FIELD_SEPARATOR = 0x1f;
const TRANSPORT_DIRECTION_PREFIX = 'beach:secure-transport:direction:';
const TRANSPORT_VERIFY_PREFIX = 'beach:secure-transport:verify:';

const encoder = new TextEncoder();

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
  const parts = [
    encoder.encode(handshakeId),
    encoder.encode(peers[0]),
    encoder.encode(peers[1]),
  ];
  const totalLength =
    parts.reduce((sum, item) => sum + item.length, 0) + (parts.length - 1);
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
  await waitForChannelOpen(channel);
  channel.binaryType = 'arraybuffer';

  const noise = await loadNoise();
  const psk = await resolvePreSharedKey(params);
  const prologue = buildPrologue(params.prologueContext);
  const handshake = createHandshake(noise, params.role, prologue, psk);
  const queue = new DataChannelQueue(channel);

  try {
    await driveHandshake(noise, handshake, queue, channel);
    const handshakeHash = handshake.GetHandshakeHash();
    const [sendCipher, recvCipher] = handshake.Split();
    // Immediately free the cipher states to avoid leaking WASM memory.
    try {
      sendCipher.free();
    } catch {}
    try {
      recvCipher.free();
    } catch {}
    const material = await deriveTransportMaterial(params, psk, handshakeHash);
    return material;
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
    return params.preSharedKey;
  }
  if (params.preSharedKeyPromise) {
    return await params.preSharedKeyPromise;
  }
  if (!params.passphrase) {
    throw new Error('secure handshake requires passphrase or pre-shared key');
  }
  return await derivePreSharedKey(params.passphrase, params.handshakeId);
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
  psk: Uint8Array,
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
  handshake.Initialize(prologue, null, null, psk);
  return handshake;
}

async function driveHandshake(
  noise: NoiseModule,
  handshake: NoiseHandshake,
  queue: DataChannelQueue,
  channel: RTCDataChannel,
): Promise<void> {
  while (true) {
    const action = handshake.GetAction();
    switch (action) {
      case noise.constants.NOISE_ACTION_WRITE_MESSAGE: {
        const message = handshake.WriteMessage(null);
        channel.send(toArrayBuffer(message));
        break;
      }
      case noise.constants.NOISE_ACTION_READ_MESSAGE: {
        const incoming = await queue.next();
        handshake.ReadMessage(incoming, false, false);
        break;
      }
      case noise.constants.NOISE_ACTION_SPLIT:
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

async function deriveTransportMaterial(
  params: BrowserHandshakeParams,
  psk: Uint8Array,
  handshakeHash: Uint8Array,
): Promise<BrowserHandshakeResult> {
  const directionOut = encoder.encode(
    `${TRANSPORT_DIRECTION_PREFIX}${params.localPeerId}->${params.remotePeerId}`,
  );
  const directionIn = encoder.encode(
    `${TRANSPORT_DIRECTION_PREFIX}${params.remotePeerId}->${params.localPeerId}`,
  );
  const peers = [params.localPeerId, params.remotePeerId].sort();
  const verifyLabel = encoder.encode(
    `${TRANSPORT_VERIFY_PREFIX}${peers[0]}|${peers[1]}`,
  );

  const sendMaterial = await hkdfExpand(handshakeHash, psk, directionOut, 32);
  const recvMaterial = await hkdfExpand(handshakeHash, psk, directionIn, 32);
  const verifyBytes = await hkdfExpand(handshakeHash, psk, verifyLabel, 4);
  const code =
    ((verifyBytes[0]! << 24) |
      (verifyBytes[1]! << 16) |
      (verifyBytes[2]! << 8) |
      verifyBytes[3]!) >>>
    0;
  const verificationCode = `${code % 1_000_000}`.padStart(6, '0');

  if (params.role === 'initiator') {
    return {
      sendKey: sendMaterial,
      recvKey: recvMaterial,
      verificationCode,
    };
  }
  return {
    sendKey: sendMaterial,
    recvKey: recvMaterial,
    verificationCode,
  };
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
    const view = new Uint8Array(
      data.buffer,
      data.byteOffset,
      data.byteLength,
    );
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
                hasHandshakeState: typeof (module as unknown as Record<string, unknown>).HandshakeState,
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
    const resolvedUrl = typeof (import.meta as any).resolve === 'function'
      ? await (import.meta as any).resolve('noise-c.wasm/src/noise-c.wasm')
      : new URL('../../../../../node_modules/noise-c.wasm/src/noise-c.wasm', import.meta.url).toString();
    const fileUrl = new URL(resolvedUrl);
    const filePath = fileURLToPath(fileUrl);
    console.debug('[beach-web][noise] resolved wasm file path', { filePath });
    const buffer = readFileSync(filePath);
    return new Uint8Array(buffer.buffer.slice(buffer.byteOffset, buffer.byteOffset + buffer.byteLength));
  } catch (error) {
    console.error('[beach-web][noise] resolveWasmBinary failed', error);
    return undefined;
  }
}
