import { WebRtcTransport, type SecureTransportSummary } from './webrtc';

export type MediaTransportEventMap = {
  frame: CustomEvent<Uint8Array>;
  status: CustomEvent<string>;
  open: Event;
  error: Event;
  close: Event;
  secure: CustomEvent<SecureTransportSummary>;
};

export interface MediaTransport extends EventTarget {
  close(): void;
}

interface DataChannelMediaTransportOptions {
  logger?: (message: string) => void;
  secureContext?: SecureTransportSummary;
}

/**
 * Thin wrapper that relays WebRTC transport events and emits raw media frames (binary payloads).
 * Text messages with a beach:status: prefix are forwarded as status events; all other
 * text payloads are ignored.
 */
export class DataChannelMediaTransport extends EventTarget implements MediaTransport {
  private readonly channel: WebRtcTransport;
  private readonly logger?: (message: string) => void;

  constructor(channel: WebRtcTransport, options: DataChannelMediaTransportOptions = {}) {
    super();
    this.channel = channel;
    this.logger = options.logger;

    this.channel.addEventListener('message', (event) => {
      const { payload } = event.detail;
      if (payload.kind === 'binary') {
        try {
          const bytes = payload.data instanceof Uint8Array ? payload.data : new Uint8Array(payload.data);
          this.dispatchEvent(new CustomEvent<Uint8Array>('frame', { detail: bytes }));
        } catch (error) {
          this.log(`failed to forward media frame: ${error instanceof Error ? error.message : String(error)}`);
          const err = new Event('error');
          Object.assign(err, { error });
          this.dispatchEvent(err);
        }
      } else if (payload.kind === 'text') {
        const text = payload.text.trim();
        if (text.startsWith('beach:status:')) {
          this.dispatchEvent(new CustomEvent<string>('status', { detail: text }));
        } else if (text === '__ready__' || text === '__offer_ready__') {
          this.log(`transport sentinel: ${text}`);
        } else {
          this.log(`ignoring unexpected text on media channel: ${text}`);
        }
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
  }

  close(): void {
    this.channel.close();
  }

  addEventListener<K extends keyof MediaTransportEventMap>(
    type: K,
    listener: (event: MediaTransportEventMap[K]) => void,
    options?: boolean | AddEventListenerOptions,
  ): void {
    super.addEventListener(type, listener as EventListener, options);
  }

  removeEventListener<K extends keyof MediaTransportEventMap>(
    type: K,
    listener: (event: MediaTransportEventMap[K]) => void,
    options?: boolean | EventListenerOptions,
  ): void {
    super.removeEventListener(type, listener as EventListener, options);
  }

  private log(message: string): void {
    if (this.logger) {
      this.logger(`[media transport] ${message}`);
    }
  }
}

export interface MediaTransport {
  addEventListener<K extends keyof MediaTransportEventMap>(
    type: K,
    listener: (event: MediaTransportEventMap[K]) => void,
    options?: boolean | AddEventListenerOptions,
  ): void;
  removeEventListener<K extends keyof MediaTransportEventMap>(
    type: K,
    listener: (event: MediaTransportEventMap[K]) => void,
    options?: boolean | EventListenerOptions,
  ): void;
}

