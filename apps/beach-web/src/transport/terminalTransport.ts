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

interface DataChannelTerminalTransportOptions {
  logger?: (message: string) => void;
}

export class DataChannelTerminalTransport extends EventTarget implements TerminalTransport {
  private readonly channel: WebRtcTransport;
  private readonly logger?: (message: string) => void;
  private framesSeen = 0;

  constructor(channel: WebRtcTransport, options: DataChannelTerminalTransportOptions = {}) {
    super();
    this.channel = channel;
    this.logger = options.logger;
    this.channel.addEventListener('message', (event) => {
      const { payload } = event.detail;
      if (payload.kind === 'binary') {
        try {
          const frame = decodeHostFrameBinary(payload.data);
          this.framesSeen += 1;
          if (this.framesSeen <= 5) {
            this.log(`received host frame #${this.framesSeen} (${frame.type})`);
          } else if (this.framesSeen % 50 === 0) {
            this.log(`received host frame #${this.framesSeen} (${frame.type})`);
          }
          this.dispatchEvent(new CustomEvent<HostFrame>('frame', { detail: frame }));
        } catch (error) {
          this.log(`failed to decode host frame: ${error instanceof Error ? error.message : String(error)}`);
          const err = new Event('error');
          Object.assign(err, { error });
          this.dispatchEvent(err);
        }
      } else {
        const text = payload.text.trim();
        if (text === '__ready__' || text === '__offer_ready__') {
          this.log(`received transport sentinel: ${text}`);
          return;
        }
        this.log(`unexpected text payload on data channel: ${text}`);
        const err = new Event('error');
        Object.assign(err, { error: new Error(`unexpected text payload: ${text}`) });
        this.dispatchEvent(err);
      }
    });

    this.channel.addEventListener('close', () => {
      this.dispatchEvent(new Event('close'));
    });
    this.channel.addEventListener('error', (event) => {
      const cloned = new Event('error');
      Object.assign(cloned, { error: (event as any).error ?? event });
      this.dispatchEvent(cloned);
    });
  }

  send(frame: ClientFrame): void {
    const encoded = encodeClientFrameBinary(frame);
    this.channel.sendBinary(encoded);
  }

  close(): void {
    this.channel.close();
  }

  private log(message: string): void {
    if (!this.logger) {
      return;
    }
    this.logger(`[terminal transport] ${message}`);
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
