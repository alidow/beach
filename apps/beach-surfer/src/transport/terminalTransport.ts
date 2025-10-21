import { decodeHostFrameBinary, encodeClientFrameBinary } from '../protocol/wire';
import type { ClientFrame, HostFrame } from '../protocol/types';
import { WebRtcTransport, type SecureTransportSummary } from './webrtc';

export type TerminalTransportEventMap = {
  frame: CustomEvent<HostFrame>;
  status: CustomEvent<string>;
  open: Event;
  error: Event;
  close: Event;
  secure: CustomEvent<SecureTransportSummary>;
};

export interface TerminalTransport extends EventTarget {
  send(frame: ClientFrame): void;
  close(): void;
}

interface DataChannelTerminalTransportOptions {
  logger?: (message: string) => void;
  secureContext?: SecureTransportSummary;
  /**
   * If provided, this binary payload will be processed immediately as if it
   * were received from the data channel. This avoids losing the initial
   * server frame (e.g. Hello) when the viewer sniffs the first message to
   * choose between terminal/media before binding this transport.
   */
  replayBinaryFirst?: Uint8Array;
}

export class DataChannelTerminalTransport extends EventTarget implements TerminalTransport {
  private readonly channel: WebRtcTransport;
  private readonly logger?: (message: string) => void;
  private framesSeen = 0;
  private readyAnnounced = false;

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
        if (text.startsWith('beach:status:')) {
          this.dispatchEvent(new CustomEvent<string>('status', { detail: text }));
          return;
        }
        this.log(`unexpected text payload on data channel: ${text}`);
      }
    });

    this.channel.addEventListener('open', () => {
      this.dispatchEvent(new Event('open'));
    });

    this.channel.addEventListener('close', () => {
      this.dispatchEvent(new Event('close'));
    });
    this.channel.addEventListener('error', (event) => {
      const cloned = new Event('error');
      Object.assign(cloned, { error: (event as any).error ?? event });
      this.dispatchEvent(cloned);
    });
    this.channel.addEventListener('secure', (event) => {
      const detail = (event as CustomEvent<SecureTransportSummary>).detail;
      this.dispatchEvent(new CustomEvent<SecureTransportSummary>('secure', { detail }));
    });
    if (options.secureContext) {
      queueMicrotask(() => {
        this.dispatchEvent(new CustomEvent<SecureTransportSummary>('secure', { detail: options.secureContext! }));
      });
    }

    // If the first terminal frame was already seen by the viewer's sniffer,
    // replay it so we don't miss the Hello and get stuck on the approval UI.
    if (options.replayBinaryFirst && options.replayBinaryFirst.length > 0) {
      const replay = () => {
        try {
          const frame = decodeHostFrameBinary(options.replayBinaryFirst!);
          this.framesSeen += 1;
          this.dispatchEvent(new CustomEvent<HostFrame>('frame', { detail: frame }));
        } catch (error) {
          this.log(
            `failed to replay initial frame: ${error instanceof Error ? error.message : String(error)}`,
          );
        }
      };
      if (typeof queueMicrotask === 'function') {
        queueMicrotask(replay);
      } else {
        Promise.resolve()
          .then(replay)
          .catch(() => {
            // The replay function already logs its own errors.
          });
      }
    }

    this.announceReadiness();
  }

  send(frame: ClientFrame): void {
    const encoded = encodeClientFrameBinary(frame);
    this.channel.sendBinary(encoded);
  }

  private announceReadiness(): void {
    if (this.readyAnnounced) {
      return;
    }

    const attempt = () => {
      if (this.readyAnnounced) {
        return;
      }
      if (!this.channel.isOpen()) {
        return;
      }
      try {
        this.channel.sendText('__ready__');
        this.readyAnnounced = true;
        this.log('sent transport sentinel: __ready__');
      } catch (error) {
        this.log(
          `failed to send readiness sentinel: ${error instanceof Error ? error.message : String(error)}`
        );
        setTimeout(attempt, 50);
      }
    };

    if (this.channel.isOpen()) {
      attempt();
    } else {
      this.channel.addEventListener(
        'open',
        () => {
          attempt();
        },
        { once: true },
      );
    }
  }

  close(): void {
    this.channel.close();
  }

  addEventListener<K extends keyof TerminalTransportEventMap>(
    type: K,
    listener: (event: TerminalTransportEventMap[K]) => void,
    options?: boolean | AddEventListenerOptions,
  ): void {
    super.addEventListener(type, listener as EventListener, options);
  }

  removeEventListener<K extends keyof TerminalTransportEventMap>(
    type: K,
    listener: (event: TerminalTransportEventMap[K]) => void,
    options?: boolean | EventListenerOptions,
  ): void {
    super.removeEventListener(type, listener as EventListener, options);
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
