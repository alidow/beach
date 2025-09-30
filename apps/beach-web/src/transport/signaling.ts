/*
 * WebSocket signaling client for negotiating terminal transports with beach-road.
 *
 * The API favours ergonomics: the client is an EventTarget subclass publishing
 * `message`, `error`, and `close` events while exposing imperative helpers for
 * sending well-typed messages.
 */

export type BuiltinTransport = 'webrtc' | 'webtransport' | 'direct';

export type TransportTypeJson = BuiltinTransport | { custom: string };

export type ClientMessage =
  | {
      type: 'join';
      peer_id: string;
      passphrase?: string | null;
      supported_transports: TransportTypeJson[];
      preferred_transport?: TransportTypeJson;
    }
  | {
      type: 'negotiate_transport';
      to_peer: string;
      proposed_transport: TransportTypeJson;
    }
  | {
      type: 'accept_transport';
      to_peer: string;
      transport: TransportTypeJson;
    }
  | {
      type: 'signal';
      to_peer: string;
      signal: unknown;
    }
  | { type: 'ping' }
  | {
      type: 'debug';
      request: unknown;
    };

export type ServerMessage =
  | {
      type: 'join_success';
      session_id: string;
      peer_id: string;
      peers: PeerInfo[];
      available_transports: TransportTypeJson[];
    }
  | {
      type: 'join_error';
      reason: string;
    }
  | {
      type: 'peer_joined';
      peer: PeerInfo;
    }
  | {
      type: 'peer_left';
      peer_id: string;
    }
  | {
      type: 'transport_proposal';
      from_peer: string;
      proposed_transport: TransportTypeJson;
    }
  | {
      type: 'transport_accepted';
      from_peer: string;
      transport: TransportTypeJson;
    }
  | {
      type: 'signal';
      from_peer: string;
      signal: unknown;
    }
  | { type: 'pong' }
  | { type: 'error'; message: string }
  | {
      type: 'debug';
      response: unknown;
    };

export interface PeerInfo {
  id: string;
  role: 'server' | 'client';
  joined_at: number;
  supported_transports: TransportTypeJson[];
  preferred_transport?: TransportTypeJson;
}

type SignalingEventMap = {
  message: CustomEvent<ServerMessage>;
  error: Event;
  close: CloseEvent;
  open: Event;
};

type WebSocketFactory = (url: string) => WebSocket;

export interface SignalingClientOptions {
  url: string;
  peerId?: string;
  passphrase?: string;
  supportedTransports?: TransportTypeJson[];
  preferredTransport?: TransportTypeJson;
  createSocket?: WebSocketFactory;
}

const DEFAULT_SUPPORTED: TransportTypeJson[] = ['webrtc'];

export class SignalingClient extends EventTarget {
  readonly peerId: string;
  private readonly socket: WebSocket;
  private readonly url: string;

  private constructor(url: string, peerId: string, socket: WebSocket) {
    super();
    this.peerId = peerId;
    this.url = url;
    this.socket = socket;
  }

  static async connect(options: SignalingClientOptions): Promise<SignalingClient> {
    const {
      url,
      peerId = generatePeerId(),
      passphrase,
      supportedTransports = DEFAULT_SUPPORTED,
      preferredTransport,
      createSocket,
    } = options;

    const factory: WebSocketFactory = createSocket ?? ((target) => new WebSocket(target));
    const socket = factory(url);
    socket.binaryType = 'arraybuffer';

    return await new Promise<SignalingClient>((resolve, reject) => {
      const handleOpen = () => {
        const client = new SignalingClient(url, peerId, socket);
        client.attachSocketListeners();
        client.send({
          type: 'join',
          peer_id: peerId,
          passphrase: passphrase ?? null,
          supported_transports: supportedTransports,
          preferred_transport: preferredTransport,
        });
        socket.removeEventListener('open', handleOpen);
        socket.removeEventListener('error', handleError);
        resolve(client);
      };

      const handleError = (event: Event) => {
        socket.removeEventListener('open', handleOpen);
        socket.removeEventListener('error', handleError);
        reject(event instanceof ErrorEvent ? event.error ?? event : event);
      };

      socket.addEventListener('open', handleOpen, { once: true });
      socket.addEventListener('error', handleError, { once: true });
    });
  }

  private attachSocketListeners(): void {
    this.socket.addEventListener('message', (event) => {
      try {
        const message = parseServerMessage(event.data);
        this.dispatchEvent(new CustomEvent<ServerMessage>('message', { detail: message }));
      } catch (error) {
        const errEvent = new Event('error');
        Object.assign(errEvent, { error });
        this.dispatchEvent(errEvent);
      }
    });

    this.socket.addEventListener('close', (event) => {
      this.dispatchEvent(new CustomEvent('close', { detail: event }));
    });

    this.socket.addEventListener('error', (event) => {
      this.dispatchEvent(new CustomEvent('error', { detail: event }));
    });

    this.socket.addEventListener('open', (event) => {
      this.dispatchEvent(new CustomEvent('open', { detail: event }));
    });
  }

  send(message: ClientMessage): void {
    try {
      this.socket.send(JSON.stringify(message));
    } catch (error) {
      const event = new Event('error');
      Object.assign(event, { error });
      this.dispatchEvent(event);
    }
  }

  ping(): void {
    this.send({ type: 'ping' });
  }

  close(code?: number, reason?: string): void {
    this.socket.close(code, reason);
  }

  async waitForMessage<T extends ServerMessage['type']>(type: T, timeoutMs = 10_000): Promise<
    Extract<ServerMessage, { type: T }>
  > {
    return await new Promise((resolve, reject) => {
      const timeout = globalThis.setTimeout(() => {
        cleanup();
        reject(new Error(`timed out waiting for signaling message "${type}"`));
      }, timeoutMs);

      const handleMessage = (event: Event) => {
        const detail = (event as CustomEvent<ServerMessage>).detail;
        if (detail.type === type) {
          cleanup();
          resolve(detail as Extract<ServerMessage, { type: T }>);
        }
      };

      const handleError = (event: Event) => {
        cleanup();
        reject(event instanceof ErrorEvent ? event.error ?? event : event);
      };

      const cleanup = () => {
        globalThis.clearTimeout(timeout);
        this.removeEventListener('message', handleMessage as EventListener);
        this.removeEventListener('error', handleError as EventListener);
        this.removeEventListener('close', handleClose as EventListener);
      };

      const handleClose = () => {
        cleanup();
        reject(new Error('signaling socket closed')); // treat as failure
      };

      this.addEventListener('message', handleMessage as EventListener);
      this.addEventListener('error', handleError as EventListener);
      this.addEventListener('close', handleClose as EventListener);
    });
  }
}

function parseServerMessage(data: unknown): ServerMessage {
  if (typeof data === 'string') {
    return normaliseServerMessage(JSON.parse(data));
  }
  if (data instanceof ArrayBuffer || ArrayBuffer.isView(data)) {
    const text = new TextDecoder().decode(data as ArrayBufferLike);
    return normaliseServerMessage(JSON.parse(text));
  }
  throw new Error('unsupported signaling payload');
}

function normaliseServerMessage(raw: any): ServerMessage {
  if (!raw || typeof raw !== 'object') {
    throw new Error('invalid signaling message payload');
  }
  switch (raw.type) {
    case 'join_success':
    case 'join_error':
    case 'peer_joined':
    case 'peer_left':
    case 'transport_proposal':
    case 'transport_accepted':
    case 'signal':
    case 'pong':
    case 'error':
    case 'debug':
      return raw as ServerMessage;
    default:
      throw new Error(`unknown signaling message type: ${raw.type}`);
  }
}

function generatePeerId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  // Fallback: RFC4122 version 4 implementation for environments without crypto.randomUUID.
  const template = 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx';
  return template.replace(/[xy]/g, (char) => {
    const random = (Math.random() * 16) | 0;
    const value = char === 'x' ? random : (random & 0x3) | 0x8;
    return value.toString(16);
  });
}

// Utility overloads for typed addEventListener/removeEventListener.
export interface SignalingClient {
  addEventListener<K extends keyof SignalingEventMap>(
    type: K,
    listener: (event: SignalingEventMap[K]) => void,
    options?: boolean | AddEventListenerOptions,
  ): void;
  removeEventListener<K extends keyof SignalingEventMap>(
    type: K,
    listener: (event: SignalingEventMap[K]) => void,
    options?: boolean | EventListenerOptions,
  ): void;
}
