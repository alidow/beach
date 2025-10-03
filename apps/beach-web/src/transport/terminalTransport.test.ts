import { describe, expect, it } from 'vitest';
import { decodeTransportMessage } from './envelope';
import { DataChannelTerminalTransport } from './terminalTransport';
import { WebRtcTransport, type DataChannelEventMap, type DataChannelLike } from './webrtc';

type SentPayload = ArrayBufferLike | ArrayBufferView | string;

class FakeDataChannel extends EventTarget implements DataChannelLike {
  readonly label = 'fake';
  readyState: RTCDataChannelState;
  binaryType: 'arraybuffer' | 'blob' = 'arraybuffer';
  readonly sent: SentPayload[] = [];

  constructor(state: RTCDataChannelState) {
    super();
    this.readyState = state;
  }

  addEventListener<K extends keyof DataChannelEventMap>(
    type: K,
    listener: (event: DataChannelEventMap[K]) => void,
    options?: boolean | AddEventListenerOptions,
  ): void {
    super.addEventListener(type, listener as EventListener, options);
  }

  removeEventListener<K extends keyof DataChannelEventMap>(
    type: K,
    listener: (event: DataChannelEventMap[K]) => void,
    options?: boolean | EventListenerOptions,
  ): void {
    super.removeEventListener(type, listener as EventListener, options);
  }

  send(data: SentPayload): void {

    this.sent.push(data);
  }

  close(): void {
    this.readyState = 'closed';
    this.dispatchEvent(new Event('close'));
  }
}

describe('DataChannelTerminalTransport readiness handshake', () => {
  it('sends the __ready__ sentinel immediately when the channel is already open', () => {
    const channel = new FakeDataChannel('open');
    const transport = new WebRtcTransport({ channel });

    new DataChannelTerminalTransport(transport);

    expect(channel.sent).toHaveLength(1);
    const encoded = channel.sent[0];
    const message = decodeTransportMessage(encoded as ArrayBufferLike);
    expect(message.payload.kind).toBe('text');
    expect(message.payload.text).toBe('__ready__');
  });

  it('waits for the open event before sending the __ready__ sentinel', () => {
    const channel = new FakeDataChannel('connecting');
    const transport = new WebRtcTransport({ channel });

    new DataChannelTerminalTransport(transport);
    expect(channel.sent).toHaveLength(0);

    channel.readyState = 'open';
    channel.dispatchEvent(new Event('open'));

    expect(channel.sent).toHaveLength(1);
    const encoded = channel.sent[0];
    const message = decodeTransportMessage(encoded as ArrayBufferLike);
    expect(message.payload.kind).toBe('text');
    expect(message.payload.text).toBe('__ready__');
  });

  it('does not send duplicate ready sentinels if the open event fires again', () => {
    const channel = new FakeDataChannel('open');
    const transport = new WebRtcTransport({ channel });

    new DataChannelTerminalTransport(transport);
    expect(channel.sent).toHaveLength(1);

    channel.dispatchEvent(new Event('open'));
    expect(channel.sent).toHaveLength(1);
  });
});
