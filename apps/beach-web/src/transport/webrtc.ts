import { decodeTransportMessage, encodeTransportMessage, type TransportMessage } from './envelope';
import type { SignalingClient, ServerMessage } from './signaling';

export type WebRtcTransportPayload = TransportMessage['payload'];

type DataChannelEventMap = {
  message: MessageEvent;
  open: Event;
  close: Event;
  error: Event;
};

export interface DataChannelLike extends EventTarget {
  readonly label: string;
  readyState: RTCDataChannelState;
  binaryType: 'arraybuffer' | 'blob';
  send(data: ArrayBufferLike | string): void;
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

export type WebRtcTransportEventMap = {
  message: CustomEvent<TransportMessage>;
  open: Event;
  close: Event;
  error: Event;
};

export interface WebRtcTransportOptions {
  channel: DataChannelLike;
  /** Optional initial outbound sequence value. Useful for deterministic tests. */
  initialSequence?: number;
}

/**
 * Lightweight wrapper around an RTCDataChannel that understands the Beach transport
 * envelope (seq + payload) and exposes a typed EventTarget API.
 */
export class WebRtcTransport extends EventTarget {
  private readonly channel: DataChannelLike;
  private sequence: number;
  private disposed = false;

  constructor(options: WebRtcTransportOptions) {
    super();
    this.channel = options.channel;
    this.channel.binaryType = 'arraybuffer';
    this.sequence = options.initialSequence ?? 0;
    this.attachChannelListeners();
  }

  /** Send a UTF-8 text payload across the data channel. Returns the assigned sequence. */
  sendText(text: string): number {
    return this.send({ kind: 'text', text });
  }

  /** Send binary payloads across the data channel. Returns the assigned sequence. */
  sendBinary(data: Uint8Array): number {
    return this.send({ kind: 'binary', data });
  }

  close(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.channel.close();
  }

  private send(payload: WebRtcTransportPayload): number {
    if (this.channel.readyState !== 'open') {
      throw new Error(`data channel ${this.channel.label} is not open`);
    }
    const message: TransportMessage = { sequence: this.sequence++, payload };
    const encoded = encodeTransportMessage(message);
    this.channel.send(encoded);
    return message.sequence;
  }

  private attachChannelListeners(): void {
    this.channel.addEventListener('open', (event) => {
      this.dispatchEvent(event);
    });

    this.channel.addEventListener('close', (event) => {
      this.dispatchEvent(event);
    });

    this.channel.addEventListener('error', (event) => {
      this.dispatchEvent(event);
    });

    this.channel.addEventListener('message', (event) => {
      try {
        const detail = decodeDataChannelPayload(event);
        this.dispatchEvent(new CustomEvent<TransportMessage>('message', { detail }));
      } catch (error) {
        const errEvent = new Event('error');
        Object.assign(errEvent, { error });
        this.dispatchEvent(errEvent);
      }
    });
  }
}

function decodeDataChannelPayload(event: MessageEvent): TransportMessage {
  const { data } = event;
  if (typeof data === 'string') {
    throw new Error('expected binary RTCDataChannel payload but received string');
  }
  if (data instanceof ArrayBuffer) {
    return decodeTransportMessage(data);
  }
  if (ArrayBuffer.isView(data)) {
    return decodeTransportMessage(data.buffer);
  }
  throw new Error('unsupported RTCDataChannel message payload');
}

export interface WebRtcTransport {
  addEventListener<K extends keyof WebRtcTransportEventMap>(
    type: K,
    listener: (event: WebRtcTransportEventMap[K]) => void,
    options?: boolean | AddEventListenerOptions,
  ): void;
  removeEventListener<K extends keyof WebRtcTransportEventMap>(
    type: K,
    listener: (event: WebRtcTransportEventMap[K]) => void,
    options?: boolean | EventListenerOptions,
  ): void;
}

export interface ConnectWebRtcTransportOptions {
  signaling: SignalingClient;
  signalingUrl: string;
  role: 'offerer' | 'answerer';
  pollIntervalMs: number;
  iceServers?: RTCIceServer[];
  preferredPeerId?: string;
  logger?: (message: string) => void;
}

export interface ConnectedWebRtcTransport {
  transport: WebRtcTransport;
  peerConnection: RTCPeerConnection;
  dataChannel: RTCDataChannel;
  remotePeerId: string;
}

export async function connectWebRtcTransport(
  options: ConnectWebRtcTransportOptions,
): Promise<ConnectedWebRtcTransport> {
  const { signaling, logger } = options;
  const join = await signaling.waitForMessage('join_success', 15_000);
  log(logger, `join_success payload: ${JSON.stringify(join)}`);
  const remotePeerId = await resolveRemotePeerId(signaling, join, options.preferredPeerId);
  log(logger, `remote peer resolved: ${remotePeerId}`);

  try {
    signaling.send({
      type: 'negotiate_transport',
      to_peer: remotePeerId,
      proposed_transport: 'webrtc',
    });
  } catch (error) {
    log(logger, `transport negotiation proposal failed: ${String(error)}`);
  }

  const pc = new RTCPeerConnection({ iceServers: options.iceServers });
  const disposeSignalListener = attachSignalListener(signaling, remotePeerId, pc, logger);
  const disposeGeneralListener = attachGeneralListener(signaling, remotePeerId, logger);

  pc.onconnectionstatechange = () => {
    log(logger, `peer connection state: ${pc.connectionState}`);
  };

  pc.onicecandidate = (event) => {
    if (!event.candidate) {
      return;
    }
    const candidate = event.candidate.toJSON();
    signaling.send({
      type: 'signal',
      to_peer: remotePeerId,
      signal: {
        transport: 'webrtc',
        signal: {
          signal_type: 'ice_candidate',
          candidate: candidate.candidate,
          sdp_mid: candidate.sdpMid ?? undefined,
          sdp_mline_index: candidate.sdpMLineIndex ?? undefined,
        },
      },
    });
  };

  try {
    if (options.role !== 'answerer') {
      throw new Error(`webrtc role ${options.role} not supported in browser client yet`);
    }
    return await connectAsAnswerer({
      pc,
      signaling,
      signalingUrl: options.signalingUrl,
      pollIntervalMs: options.pollIntervalMs,
      remotePeerId,
      logger,
    });
  } finally {
    disposeSignalListener();
    disposeGeneralListener();
  }
}

function attachSignalListener(
  signaling: SignalingClient,
  remotePeerId: string,
  pc: RTCPeerConnection,
  logger?: (message: string) => void,
): () => void {
  const handler = (event: Event) => {
    const detail = (event as CustomEvent<ServerMessage>).detail;
    if (detail.type === 'signal') {
      log(logger, `signal message: ${JSON.stringify(detail.signal)}`);
    }
    if (detail.type !== 'signal' || detail.from_peer !== remotePeerId) {
      return;
    }
    const signal = parseWebRtcSignal(detail.signal);
    if (!signal) {
      return;
    }
    if (signal.signal.signal_type === 'ice_candidate') {
      const candidate: RTCIceCandidateInit = {
        candidate: signal.signal.candidate,
        sdpMid: signal.signal.sdp_mid ?? undefined,
        sdpMLineIndex: signal.signal.sdp_mline_index ?? undefined,
      };
      log(logger, 'received remote ice candidate');
      pc.addIceCandidate(candidate).catch((error) => log(logger, `ice add failed: ${error}`));
    }
  };

  signaling.addEventListener('message', handler as EventListener);
  return () => signaling.removeEventListener('message', handler as EventListener);
}

async function resolveRemotePeerId(
  signaling: SignalingClient,
  join: Extract<ServerMessage, { type: 'join_success' }>,
  preferred?: string,
): Promise<string> {
  if (preferred) {
    const match = join.peers.find((peer) => peer.id === preferred);
    if (match) {
      return preferred;
    }
  }
  const existing = join.peers.find((peer) => peer.role === 'server');
  if (existing) {
    return existing.id;
  }
  return await new Promise<string>((resolve) => {
    const handler = (event: Event) => {
      const detail = (event as CustomEvent<ServerMessage>).detail;
      if (detail.type === 'peer_joined' && detail.peer.role === 'server') {
        signaling.removeEventListener('message', handler as EventListener);
        resolve(detail.peer.id);
      }
    };
    signaling.addEventListener('message', handler as EventListener);
  });
}

function parseWebRtcSignal(raw: unknown):
  | { transport: 'webrtc'; signal: { signal_type: 'offer' | 'answer'; sdp: string } }
  | {
      transport: 'webrtc';
      signal: {
        signal_type: 'ice_candidate';
        candidate: string;
        sdp_mid?: string;
        sdp_mline_index?: number;
      };
    }
  | undefined {
  if (!raw || typeof raw !== 'object') {
    return undefined;
  }
  const transport = (raw as any).transport;
  if (transport !== 'webrtc') {
    return undefined;
  }
  const signal = (raw as any).signal;
  if (!signal || typeof signal !== 'object') {
    return undefined;
  }
  const signalType = signal.signal_type;
  if (signalType === 'offer' || signalType === 'answer') {
    return {
      transport: 'webrtc',
      signal: {
        signal_type: signalType,
        sdp: typeof signal.sdp === 'string' ? signal.sdp : '',
      },
    };
  }
  if (signalType === 'ice_candidate') {
    if (typeof signal.candidate !== 'string') {
      return undefined;
    }
    return {
      transport: 'webrtc',
      signal: {
        signal_type: 'ice_candidate',
        candidate: signal.candidate,
        sdp_mid: typeof signal.sdp_mid === 'string' ? signal.sdp_mid : undefined,
        sdp_mline_index:
          typeof signal.sdp_mline_index === 'number' ? signal.sdp_mline_index : undefined,
      },
    };
  }
  return undefined;
}

async function connectAsAnswerer(options: {
  pc: RTCPeerConnection;
  signaling: SignalingClient;
  signalingUrl: string;
  pollIntervalMs: number;
  remotePeerId: string;
  logger?: (message: string) => void;
}): Promise<ConnectedWebRtcTransport> {
  const { pc, signalingUrl, pollIntervalMs, remotePeerId, logger } = options;
  log(logger, 'polling for SDP offer');
  const offer = await pollSdp(`${signalingUrl.replace(/\/$/, '')}/offer`, pollIntervalMs, logger);
  log(logger, 'SDP offer received');
  const channelPromise = waitForDataChannel(pc, remotePeerId, logger);

  await pc.setRemoteDescription({ type: offer.type as RTCSdpType, sdp: offer.sdp });

  const answer = await pc.createAnswer();
  await pc.setLocalDescription(answer);
  await postSdp(`${signalingUrl.replace(/\/$/, '')}/answer`, {
    sdp: answer.sdp ?? '',
    type: answer.type,
  });
  log(logger, 'SDP answer posted');

  return await channelPromise;
}

async function waitForDataChannel(
  pc: RTCPeerConnection,
  remotePeerId: string,
  logger?: (message: string) => void,
): Promise<ConnectedWebRtcTransport> {
  return await new Promise<ConnectedWebRtcTransport>((resolve, reject) => {
    const timeout = setTimeout(() => {
      reject(new Error('timed out waiting for data channel'));
    }, 20_000);

    pc.ondatachannel = (event) => {
      const channel = event.channel;
      log(logger, `data channel announced: ${channel.label}`);
      channel.binaryType = 'arraybuffer';
      const transport = new WebRtcTransport({ channel });

      const handleOpen = () => {
        cleanup();
        transport.sendText('__ready__');
        log(logger, 'data channel open');
        resolve({
          transport,
          peerConnection: pc,
          dataChannel: channel,
          remotePeerId,
        });
      };

      const handleError = (event: Event) => {
        cleanup();
        reject((event as any).error ?? new Error('data channel error'));
      };

      const cleanup = () => {
        clearTimeout(timeout);
        channel.removeEventListener('open', handleOpen);
        channel.removeEventListener('error', handleError);
      };

      channel.addEventListener('open', handleOpen, { once: true });
      channel.addEventListener('error', handleError, { once: true });
      channel.addEventListener('close', () => log(logger, 'data channel closed'));
    };
  });
}

async function pollSdp(
  url: string,
  pollIntervalMs: number,
  logger?: (message: string) => void,
): Promise<WebRtcSdpPayload> {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const payload = await fetchSdp(url);
    if (payload) {
      return payload;
    }
    await delay(pollIntervalMs);
  }
  throw new Error('timed out waiting for SDP payload');
}

async function fetchSdp(url: string): Promise<WebRtcSdpPayload | null> {
  const response = await fetch(url, { cache: 'no-cache' });
  if (response.status === 404) {
    return null;
  }
  if (!response.ok) {
    throw new Error(`signaling fetch failed: ${response.status} ${response.statusText}`);
  }
  return await response.json();
}

async function postSdp(url: string, payload: WebRtcSdpPayload): Promise<void> {
  const response = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });
  if (!response.ok && response.status !== 204) {
    throw new Error(`signaling post failed: ${response.status} ${response.statusText}`);
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

interface WebRtcSdpPayload {
  sdp: string;
  type: string;
}

function log(logger: ((message: string) => void) | undefined, message: string): void {
  if (logger) {
    logger(message);
  } else {
    console.log(`[beach-web] ${message}`);
  }
}

function attachGeneralListener(
  signaling: SignalingClient,
  remotePeerId: string,
  logger?: (message: string) => void,
): () => void {
  const handler = (event: Event) => {
    const detail = (event as CustomEvent<ServerMessage>).detail;
    if (detail.type === 'transport_proposal' && detail.from_peer === remotePeerId) {
      log(logger, `transport proposal received: ${JSON.stringify(detail.proposed_transport)}`);
      signaling.send({
        type: 'accept_transport',
        to_peer: remotePeerId,
        transport: detail.proposed_transport,
      });
    }
  };
  signaling.addEventListener('message', handler as EventListener);
  return () => signaling.removeEventListener('message', handler as EventListener);
}
