import { decodeHostFrameBinary, encodeClientFrameBinary } from '../protocol/wire';
import type { ClientFrame, HostFrame } from '../protocol/types';
import { WebRtcTransport } from './webrtc';

export type TerminalTransportEventMap = {
  frame: CustomEvent<HostFrame>;
  error: Event;
  close: Event;
};

export interface TerminalTransport extends EventTarget {
  send(frame: ClientFrame): void;
  close(): void;
}

export class DataChannelTerminalTransport extends EventTarget implements TerminalTransport {
  private readonly channel: WebRtcTransport;

  constructor(channel: WebRtcTransport) {
    super();
    this.channel = channel;
    this.channel.addEventListener('message', (event) => {
      const { payload } = event.detail;
      if (payload.kind === 'binary') {
        try {
          const frame = decodeHostFrameBinary(payload.data);
          this.dispatchEvent(new CustomEvent<HostFrame>('frame', { detail: frame }));
        } catch (error) {
          const err = new Event('error');
          Object.assign(err, { error });
          this.dispatchEvent(err);
        }
      } else {
        const text = payload.text.trim();
        if (text === '__ready__' || text === '__offer_ready__') {
          return;
        }
        const err = new Event('error');
        Object.assign(err, { error: new Error(`unexpected text payload: ${text}`) });
        this.dispatchEvent(err);
      }
    });

    this.channel.addEventListener('close', (event) => this.dispatchEvent(event));
    this.channel.addEventListener('error', (event) => this.dispatchEvent(event));
  }

  send(frame: ClientFrame): void {
    const encoded = encodeClientFrameBinary(frame);
    this.channel.sendBinary(encoded);
  }

  close(): void {
    this.channel.close();
  }
}

export interface TerminalTransport {
  addEventListener<K extends keyof TerminalTransportEventMap>(
    type: K,
    listener: (event: TerminalTransportEventMap[K]) => void,
    options?: boolean | AddEventListenerOptions,
  ): void;
  removeEventListener<K extends keyof TerminalTransportEventMap>(
    type: K,
    listener: (event: TerminalTransportEventMap[K]) => void,
    options?: boolean | EventListenerOptions,
  ): void;
}
