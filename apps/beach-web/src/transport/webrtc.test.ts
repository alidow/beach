import { describe, expect, it } from 'vitest';
import { decodeTransportMessage } from './envelope';
import { WebRtcTransport } from './webrtc';

describe('WebRtcTransport', () => {
  it('encodes outgoing text payloads with transport envelope', () => {
    const channel = new MockDataChannel();
    const transport = new WebRtcTransport({ channel, initialSequence: 7 });
    channel.simulateOpen();

    const seq = transport.sendText('hello');
    expect(seq).toBe(7);
    expect(channel.sent.length).toBe(1);

    const message = decodeTransportMessage(channel.sent[0]!);
    expect(message.sequence).toBe(7);
    expect(message.payload).toEqual({ kind: 'text', text: 'hello' });
  });

  it('dispatches inbound messages decoded from the envelope', () => {
    const channel = new MockDataChannel();
    const transport = new WebRtcTransport({ channel });

    const events: unknown[] = [];
    transport.addEventListener('message', (event) => events.push(event.detail));

    const encoded = new Uint8Array([
      1, // payload type binary
      0, 0, 0, 0, 0, 0, 0, 1, // sequence 1
      0, 0, 0, 2, // length 2
      0xde,
      0xad,
    ]);
    channel.simulateMessage(encoded.buffer);

    expect(events).toHaveLength(1);
    const [detail] = events as any[];
    expect(detail.sequence).toBe(1);
    expect(detail.payload.kind).toBe('binary');
    if (detail.payload.kind === 'binary') {
      expect(detail.payload.data).toEqual(Uint8Array.from([0xde, 0xad]));
    }
  });
});

class MockDataChannel extends EventTarget {
  readonly label = 'mock';
  readyState: RTCDataChannelState = 'connecting';
  binaryType: 'arraybuffer' | 'blob' = 'arraybuffer';
  sent: ArrayBufferLike[] = [];

  send(data: ArrayBufferLike | string): void {
    if (typeof data === 'string') {
      throw new Error('expected binary payload');
    }
    this.sent.push(data);
  }

  close(): void {
    this.dispatchEvent(new Event('close'));
  }

  simulateOpen(): void {
    this.readyState = 'open';
    this.dispatchEvent(new Event('open'));
  }

  simulateMessage(data: ArrayBufferLike): void {
    const event = new MessageEvent('message', { data });
    this.dispatchEvent(event);
  }
}
