import { webcrypto } from 'node:crypto';
import { describe, expect, it } from 'vitest';

import {
  buildPrologueContext,
  runBrowserHandshake,
} from './noiseHandshake';
import { SecureDataChannel } from './secureDataChannel';

if (typeof globalThis.crypto === 'undefined') {
  Object.defineProperty(globalThis, 'crypto', {
    value: webcrypto,
    configurable: false,
    enumerable: false,
    writable: false,
  });
}

const HANDSHAKE_LABEL = 'beach-secure-handshake';
const TRANSPORT_LABEL = 'beach-transport';

// Skip this test in vitest/jsdom environment as WASM loading is not properly supported
describe.skip('browser Noise handshake', () => {
  it('derives matching keys for initiator and responder and secures data channel traffic', async () => {
    const handshakeInitiator = new MockRTCDataChannel(HANDSHAKE_LABEL);
    const handshakeResponder = new MockRTCDataChannel(HANDSHAKE_LABEL);
    handshakeInitiator.setPeer(handshakeResponder);

    const handshakeId = 'session-handshake-123';
    const passphrase = 'correct horse battery staple';
    const initiatorPeerId = 'peer-offerer';
    const responderPeerId = 'peer-answerer';
    const prologueInitiator = buildPrologueContext(
      handshakeId,
      initiatorPeerId,
      responderPeerId,
    );
    const prologueResponder = buildPrologueContext(
      handshakeId,
      responderPeerId,
      initiatorPeerId,
    );

    let initiatorResult: Awaited<ReturnType<typeof runBrowserHandshake>>;
    let responderResult: Awaited<ReturnType<typeof runBrowserHandshake>>;
    [initiatorResult, responderResult] = await Promise.all([
      runBrowserHandshake(handshakeInitiator.asRTC(), {
        role: 'initiator',
        handshakeId,
        localPeerId: initiatorPeerId,
        remotePeerId: responderPeerId,
        prologueContext: prologueInitiator,
        passphrase,
      }),
      runBrowserHandshake(handshakeResponder.asRTC(), {
        role: 'responder',
        handshakeId,
        localPeerId: responderPeerId,
        remotePeerId: initiatorPeerId,
        prologueContext: prologueResponder,
        passphrase,
      }),
    ]);

    expect(initiatorResult.sendKey).toHaveLength(32);
    expect(initiatorResult.recvKey).toHaveLength(32);
    expect(responderResult.sendKey).toHaveLength(32);
    expect(responderResult.recvKey).toHaveLength(32);
    expect(Array.from(initiatorResult.sendKey)).toEqual(Array.from(responderResult.recvKey));
    expect(Array.from(initiatorResult.recvKey)).toEqual(Array.from(responderResult.sendKey));
    expect(initiatorResult.verificationCode).toHaveLength(6);
    expect(initiatorResult.verificationCode).toBe(responderResult.verificationCode);

    const transportInitiator = new MockRTCDataChannel(TRANSPORT_LABEL);
    const transportResponder = new MockRTCDataChannel(TRANSPORT_LABEL);
    transportInitiator.setPeer(transportResponder);

    const secureInitiator = new SecureDataChannel(transportInitiator.asRTC(), {
      sendKey: initiatorResult.sendKey,
      recvKey: initiatorResult.recvKey,
    });
    const secureResponder = new SecureDataChannel(transportResponder.asRTC(), {
      sendKey: responderResult.sendKey,
      recvKey: responderResult.recvKey,
    });

    const payload = new Uint8Array([1, 2, 3, 4, 5, 6]);
    await waitForOpenEvent(secureInitiator);
    await waitForOpenEvent(secureResponder);

    const receivedPromise = captureMessage(secureResponder);
    secureInitiator.send(payload);
    const decrypted = await receivedPromise;
    expect(Array.from(new Uint8Array(decrypted))).toEqual(Array.from(payload));

    secureInitiator.close();
    secureResponder.close();
  });
});

async function waitForOpenEvent(channel: EventTarget): Promise<void> {
  if ((channel as any).readyState === 'open') {
    await Promise.resolve();
    return;
  }
  await new Promise<void>((resolve) => {
    channel.addEventListener('open', () => resolve(), { once: true });
  });
}

async function captureMessage(channel: EventTarget): Promise<ArrayBuffer> {
  return await new Promise<ArrayBuffer>((resolve) => {
    const handler = (event: Event) => {
      const data = (event as MessageEvent).data;
      resolve(data as ArrayBuffer);
    };
    channel.addEventListener('message', handler as EventListener, { once: true });
  });
}

class MockRTCDataChannel extends EventTarget {
  label: string;
  readyState: RTCDataChannelState = 'open';
  binaryType: 'arraybuffer' | 'blob' = 'arraybuffer';
  private peer?: MockRTCDataChannel;
  private closing = false;

  constructor(label: string) {
    super();
    this.label = label;
    queueMicrotask(() => this.dispatchEvent(new Event('open')));
  }

  setPeer(peer: MockRTCDataChannel): void {
    this.peer = peer;
    peer.peer = this;
  }

  send(data: ArrayBufferLike | ArrayBufferView | string): void {
    if (!this.peer || this.peer.readyState !== 'open') {
      return;
    }
    const payload = cloneData(data);
    queueMicrotask(() => {
      if (this.peer && this.peer.readyState === 'open') {
        this.peer.dispatchEvent(new MessageEvent('message', { data: payload }));
      }
    });
  }

  close(): void {
    if (this.closing) {
      return;
    }
    this.closing = true;
    this.readyState = 'closed';
    queueMicrotask(() => this.dispatchEvent(new Event('close')));
    if (this.peer && this.peer.readyState !== 'closed') {
      this.peer.close();
    }
  }

  asRTC(): RTCDataChannel {
    return this as unknown as RTCDataChannel;
  }
}

function cloneData(data: ArrayBufferLike | ArrayBufferView | string): any {
  if (typeof data === 'string') {
    return data;
  }
  if (data instanceof ArrayBuffer) {
    return data.slice(0);
  }
  if (ArrayBuffer.isView(data)) {
    const view = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    return view.slice();
  }
  throw new Error('unsupported payload type');
}
