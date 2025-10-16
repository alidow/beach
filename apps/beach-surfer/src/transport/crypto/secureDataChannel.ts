import { chacha20poly1305Decrypt, chacha20poly1305Encrypt } from './chachaPoly';

const TRANSPORT_VERSION = 1;
const TRANSPORT_AAD = new TextEncoder().encode('beach:secure-transport:v1');

export interface SecureChannelKeys {
  sendKey: Uint8Array;
  recvKey: Uint8Array;
}

export interface DataChannelLike {
  readonly label: string;
  readyState: RTCDataChannelState;
  binaryType: 'arraybuffer' | 'blob';
  send(data: ArrayBufferLike | ArrayBufferView | string): void;
  close(): void;
  addEventListener<K extends keyof DataChannelEventMap>(
    type: K,
    listener: (event: DataChannelEventMap[K]) => void,
    options?: boolean | AddEventListenerOptions,
  ): void;
  removeEventListener<K extends keyof DataChannelEventMap>(
    type: K,
    listener: (event: DataChannelEventMap[K]) => void,
    options?: boolean | EventListenerOptions,
  ): void;
}

export interface DataChannelEventMap {
  message: MessageEvent;
  open: Event;
  close: Event;
  error: Event;
}

export class SecureDataChannel extends EventTarget implements DataChannelLike {
  readonly label: string;
  private readonly channel: RTCDataChannel;
  private readonly sendKey: Uint8Array;
  private readonly recvKey: Uint8Array;
  private sendCounter = 0n;
  private recvCounter = 0n;
  private seenOpen = false;
  private binaryTypeInternal: 'arraybuffer' | 'blob' = 'arraybuffer';

  private readonly handleMessage = (event: MessageEvent) => {
    try {
      const frame = normaliseData(event.data);
      const plaintext = this.decryptFrame(frame);
      const payload = plaintext.buffer.slice(
        plaintext.byteOffset,
        plaintext.byteOffset + plaintext.byteLength,
      );
      this.dispatchEvent(new MessageEvent('message', { data: payload }));
    } catch (error) {
      const errEvent = new Event('error');
      Object.assign(errEvent, {
        error: error instanceof Error ? error : new Error(String(error)),
      });
      this.dispatchEvent(errEvent);
    }
  };

  private readonly handleOpen = () => {
    this.seenOpen = true;
    this.dispatchEvent(new Event('open'));
  };

  private readonly handleClose = () => {
    this.dispatchEvent(new Event('close'));
  };

  private readonly handleError = (event: Event) => {
    const errEvent = new Event('error');
    Object.assign(errEvent, { error: (event as any).error ?? event });
    this.dispatchEvent(errEvent);
  };

  constructor(channel: RTCDataChannel, keys: SecureChannelKeys) {
    super();
    this.channel = channel;
    this.label = channel.label;
    this.sendKey = keys.sendKey;
    this.recvKey = keys.recvKey;
    channel.binaryType = 'arraybuffer';
    channel.addEventListener('message', this.handleMessage);
    channel.addEventListener('open', this.handleOpen);
    channel.addEventListener('close', this.handleClose);
    channel.addEventListener('error', this.handleError);
    if (channel.readyState === 'open') {
      queueMicrotask(() => {
        if (!this.seenOpen) {
          this.handleOpen();
        }
      });
    }
  }

  get readyState(): RTCDataChannelState {
    return this.channel.readyState;
  }

  set readyState(_state: RTCDataChannelState) {
    throw new Error('readyState is read-only');
  }

  get binaryType(): 'arraybuffer' | 'blob' {
    return this.binaryTypeInternal;
  }

  set binaryType(value: 'arraybuffer' | 'blob') {
    this.binaryTypeInternal = value;
    this.channel.binaryType = value;
  }

  send(data: ArrayBufferLike | ArrayBufferView | string): void {
    if (this.channel.readyState !== 'open') {
      throw new Error(`data channel ${this.label} is not open`);
    }
    const plaintext = coercePayload(data);
    const counter = this.sendCounter;
    this.sendCounter += 1n;
    const nonce = buildNonce(counter);
    const ciphertext = chacha20poly1305Encrypt(this.sendKey, nonce, TRANSPORT_AAD, plaintext);
    const frame = buildFrame(counter, ciphertext);
    const payload = toArrayBuffer(frame);
    this.channel.send(payload);
  }

  close(): void {
    this.channel.removeEventListener('message', this.handleMessage);
    this.channel.removeEventListener('open', this.handleOpen);
    this.channel.removeEventListener('close', this.handleClose);
    this.channel.removeEventListener('error', this.handleError);
    this.channel.close();
  }

  addEventListener<K extends keyof DataChannelEventMap>(
    type: K,
    listener: (event: DataChannelEventMap[K]) => void,
    options?: boolean | AddEventListenerOptions,
  ): void {
    super.addEventListener(type, listener as EventListenerOrEventListenerObject, options);
  }

  removeEventListener<K extends keyof DataChannelEventMap>(
    type: K,
    listener: (event: DataChannelEventMap[K]) => void,
    options?: boolean | EventListenerOptions,
  ): void {
    super.removeEventListener(type, listener as EventListenerOrEventListenerObject, options);
  }

  private decryptFrame(frame: Uint8Array): Uint8Array {
    if (frame.length < 9) {
      throw new Error('secure transport frame too short');
    }
    const version = frame[0];
    if (version !== TRANSPORT_VERSION) {
      throw new Error(`secure transport version mismatch (${version})`);
    }
    const counter = readCounter(frame.subarray(1, 9));
    if (counter !== this.recvCounter) {
      throw new Error(`secure transport counter mismatch (expected ${this.recvCounter}, got ${counter})`);
    }
    const nonce = buildNonce(counter);
    const plaintext = chacha20poly1305Decrypt(
      this.recvKey,
      nonce,
      TRANSPORT_AAD,
      frame.subarray(9),
    );
    this.recvCounter += 1n;
    return plaintext;
  }
}

function coercePayload(payload: ArrayBufferLike | ArrayBufferView | string): Uint8Array {
  if (typeof payload === 'string') {
    return new TextEncoder().encode(payload);
  }
  if (payload instanceof ArrayBuffer) {
    return new Uint8Array(payload);
  }
  if (ArrayBuffer.isView(payload)) {
    const view = new Uint8Array(payload.buffer, payload.byteOffset, payload.byteLength);
    return view.slice();
  }
  throw new Error('unsupported payload type for secure transport');
}

function buildFrame(counter: bigint, ciphertext: Uint8Array): Uint8Array {
  const frame = new Uint8Array(1 + 8 + ciphertext.length);
  frame[0] = TRANSPORT_VERSION;
  const counterView = new DataView(frame.buffer, 1, 8);
  counterView.setBigUint64(0, counter, false);
  frame.set(ciphertext, 9);
  return frame;
}

function buildNonce(counter: bigint): Uint8Array {
  const nonce = new Uint8Array(12);
  const view = new DataView(nonce.buffer, 4, 8);
  view.setBigUint64(0, counter, false);
  return nonce;
}

function readCounter(bytes: Uint8Array): bigint {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return view.getBigUint64(0, false);
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
