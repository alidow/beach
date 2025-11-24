import { describe, expect, it, vi } from 'vitest';
import { decodeTransportMessage, encodeTransportMessage } from './envelope';
import {
  WebRtcTransport,
  appendParams,
  pollSdp,
  postSdp,
} from './webrtc';
import {
  encodeFramedMessage,
  decodeFramedMessage,
  FramedReassembler,
  runtimeFramingConfig,
} from './chunk';

describe('WebRtcTransport', () => {
  it('encodes outgoing text payloads with transport envelope', () => {
    const channel = new MockDataChannel();
    const transport = new WebRtcTransport({ channel, initialSequence: 7 });
    channel.simulateOpen();

    const seq = transport.sendText('hello');
    expect(seq).toBe(7);
    expect(channel.sent.length).toBe(1);

    const framed = channel.sent[0]!;
    const decodedFrame = decodeFramedMessage(
      new Uint8Array(framed as ArrayBufferLike),
      new FramedReassembler(),
      runtimeFramingConfig(),
      Date.now(),
    );
    if (decodedFrame.kind !== 'complete') {
      throw new Error('expected complete frame');
    }
    const message = decodeTransportMessage(decodedFrame.frame.payload);
    expect(message.sequence).toBe(7);
    expect(message.payload).toEqual({ kind: 'text', text: 'hello' });
  });

  it('dispatches inbound messages decoded from the envelope', () => {
    const channel = new MockDataChannel();
    const transport = new WebRtcTransport({ channel });

    const events: unknown[] = [];
    transport.addEventListener('message', (event) => events.push(event.detail));

    const envelope = encodeTransportMessage({ sequence: 1, payload: { kind: 'binary', data: new Uint8Array([0xde, 0xad]) } });
    const framed = encodeFramedMessage(
      'sync',
      'binary',
      1,
      envelope,
      runtimeFramingConfig(),
    )[0];
    channel.simulateMessage(framed);

    expect(events).toHaveLength(1);
    const [detail] = events as any[];
    expect(detail.sequence).toBe(1);
    expect(detail.payload.kind).toBe('binary');
    if (detail.payload.kind === 'binary') {
      expect(detail.payload.data).toEqual(Uint8Array.from([0xde, 0xad]));
    }
  });
});

describe('appendParams helper', () => {
  it(
    'adds query parameters without clobbering existing ones',
    () => {
      const base = 'http://127.0.0.1/offer?existing=value';
      const result = appendParams(base, { peer_id: 'abc', handshake_id: '123' });
      const url = new URL(result);
      expect(url.searchParams.get('existing')).toBe('value');
      expect(url.searchParams.get('peer_id')).toBe('abc');
      expect(url.searchParams.get('handshake_id')).toBe('123');
    },
    60_000,
  );

  it(
    'returns the original url when params are absent',
    () => {
      const base = 'http://127.0.0.1/answer';
      const result = appendParams(base, undefined);
      expect(result).toBe(base);
    },
    60_000,
  );
});

describe('signaling helpers', () => {
  it(
    'pollSdp retries on 409 with Retry-After',
    async () => {
      const fetchMock = vi.fn();
      let calls = 0;
      (global as any).fetch = fetchMock;
      fetchMock.mockImplementation(async () => {
        calls += 1;
        if (calls === 1) {
          return new Response(null, {
            status: 409,
            headers: new Headers({ 'Retry-After': '0.01' }),
          });
        }
        return new Response(
          JSON.stringify({
            sdp: 'v=0',
            type: 'offer',
            handshake_id: 'h1',
            from_peer: 'a',
            to_peer: 'b',
          }),
          { status: 200, headers: { 'Content-Type': 'application/json' } },
        );
      });
      vi.useFakeTimers();
      const promise = pollSdp('http://example.invalid/offer', 5, undefined);
      await vi.runAllTimersAsync();
      const payload = await promise;
      expect(payload.handshake_id).toBe('h1');
      vi.useRealTimers();
    },
    10_000,
  );

  it(
    'postSdp retries on 409',
    async () => {
      const fetchMock = vi.fn();
      (global as any).fetch = fetchMock;
      fetchMock
        .mockResolvedValueOnce(new Response(null, { status: 409 }))
        .mockResolvedValueOnce(new Response(null, { status: 204 }));
      await postSdp('http://example.invalid/answer', {
        sdp: 'v=0',
        type: 'answer',
        handshake_id: 'h2',
        from_peer: 'a',
        to_peer: 'b',
      });
      expect(fetchMock).toHaveBeenCalledTimes(2);
    },
    10_000,
  );
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
