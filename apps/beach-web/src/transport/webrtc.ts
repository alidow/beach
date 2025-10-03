import { decodeTransportMessage, encodeTransportMessage, type TransportMessage } from './envelope';
import type { SignalingClient, ServerMessage } from './signaling';

export type WebRtcTransportPayload = TransportMessage['payload'];

export type DataChannelEventMap = {
  message: MessageEvent;
  open: Event;
  close: Event;
  error: Event;
};

export interface DataChannelLike extends EventTarget {
  readonly label: string;
  readyState: RTCDataChannelState;
  binaryType: 'arraybuffer' | 'blob';
  send(data: ArrayBufferLike | ArrayBufferView | string): void;
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
  private open: boolean;

  constructor(options: WebRtcTransportOptions) {
    super();
    this.channel = options.channel;
    this.channel.binaryType = 'arraybuffer';
    this.sequence = options.initialSequence ?? 0;
    this.open = this.channel.readyState === 'open';
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
    this.open = false;
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

  isOpen(): boolean {
    return this.open && this.channel.readyState === 'open';
  }

  private attachChannelListeners(): void {
    // Never re-dispatch the same event object that's currently being dispatched
    // by the underlying RTCDataChannel — doing so can throw InvalidStateError.
    this.channel.addEventListener('open', () => {
      this.open = true;
      this.dispatchEvent(new Event('open'));
    });

    this.channel.addEventListener('close', () => {
      this.open = false;
      this.dispatchEvent(new Event('close'));
    });

    this.channel.addEventListener('error', (event) => {
      const cloned = new Event('error');
      // Preserve error detail if present on the original event
      Object.assign(cloned, { error: (event as any).error ?? event });
      this.dispatchEvent(cloned);
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
    const view = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    return decodeTransportMessage(view);
  }
  throw new Error('unsupported RTCDataChannel message payload');
}

export interface WebRtcTransport {
  isOpen(): boolean;
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

const ANSWER_FLUSH_DELAY_MS = 400;

export async function connectWebRtcTransport(
  options: ConnectWebRtcTransportOptions,
): Promise<ConnectedWebRtcTransport> {
  const { signaling, logger } = options;
  const join = await signaling.waitForMessage('join_success', 15_000);
  log(logger, `join_success payload: ${JSON.stringify(join)}`);
  const assignedPeerId = join.peer_id;
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

  const pc = new RTCPeerConnection({
    iceServers:
      options.iceServers && options.iceServers.length > 0
        ? options.iceServers
        : [{ urls: 'stun:stun.l.google.com:19302' }],
  });
  // Queue remote ICE candidates until the remote description is set.
  let remoteDescriptionSet = false;
  const pendingRemoteCandidates: RTCIceCandidateInit[] = [];
  const onRemoteCandidate = (cand: RTCIceCandidateInit) => {
    if (!remoteDescriptionSet) {
      pendingRemoteCandidates.push(cand);
      return;
    }
    pc
      .addIceCandidate(cand)
      .then(() => log(logger, `ice add ok: ${(cand.candidate ?? '').slice(0, 80)}`))
      .catch((error) => log(logger, `ice add failed: ${error}`));
  };
  let currentHandshakeId: string | null = null;
  const disposeSignalListener = attachSignalListener(
    signaling,
    remotePeerId,
    () => currentHandshakeId,
    onRemoteCandidate,
    logger,
  );
  const disposeGeneralListener = attachGeneralListener(signaling, remotePeerId, logger);

  pc.onconnectionstatechange = () => {
    log(logger, `peer connection state: ${pc.connectionState}`);
  };
  pc.onsignalingstatechange = () => {
    log(logger, `signaling state: ${pc.signalingState}`);
  };
  pc.oniceconnectionstatechange = () => {
    log(logger, `ice connection state: ${pc.iceConnectionState}`);
  };
  const pendingLocalCandidates: RTCIceCandidateInit[] = [];
  const allLocalCandidates: RTCIceCandidateInit[] = [];
  let candidateSendState: 'blocked' | 'delayed' | 'ready' = 'blocked';
  let flushTimer: ReturnType<typeof setTimeout> | null = null;
  let resendTimer: ReturnType<typeof setTimeout> | null = null;
  let resendAttempts = 0;
  const MAX_RESEND_ATTEMPTS = 3;
  const RESEND_INTERVAL_MS = 1200;

  const dispatchCandidate = (candidate: RTCIceCandidateInit) => {
    if (!currentHandshakeId) {
      pendingLocalCandidates.unshift(candidate);
      return;
    }
    log(logger, `sending local candidate: ${JSON.stringify(candidate)}`);
    signaling.send({
      type: 'signal',
      to_peer: remotePeerId,
      signal: {
        transport: 'webrtc',
        signal: {
          signal_type: 'ice_candidate',
          handshake_id: currentHandshakeId,
          candidate: candidate.candidate ?? '',
          sdp_mid: candidate.sdpMid ?? undefined,
          sdp_mline_index: candidate.sdpMLineIndex ?? undefined,
        },
      },
    });
  };

  const flushPendingCandidates = () => {
    if (pendingLocalCandidates.length === 0 || !currentHandshakeId) {
      return;
    }
    while (pendingLocalCandidates.length > 0) {
      const candidate = pendingLocalCandidates.shift()!;
      dispatchCandidate(candidate);
    }
    scheduleResend();
  };

  const scheduleFlush = (delayMs: number) => {
    if (flushTimer !== null) {
      return;
    }
    flushTimer = setTimeout(() => {
      flushTimer = null;
      candidateSendState = 'ready';
      flushPendingCandidates();
    }, delayMs);
  };

  const scheduleResend = () => {
    if (candidateSendState !== 'ready') {
      return;
    }
    if (resendTimer !== null || resendAttempts >= MAX_RESEND_ATTEMPTS) {
      return;
    }
    resendTimer = setTimeout(() => {
      resendTimer = null;
      resendAttempts += 1;
      for (const candidate of allLocalCandidates) {
        dispatchCandidate({ ...candidate });
      }
      if (resendAttempts < MAX_RESEND_ATTEMPTS) {
        scheduleResend();
      }
    }, RESEND_INTERVAL_MS);
  };

  pc.onicegatheringstatechange = () => {
    log(logger, `ice gathering state: ${pc.iceGatheringState}`);
  };

  pc.onicecandidate = (event) => {
    if (!event.candidate) {
      return;
    }
    const candidate = event.candidate.toJSON();
    log(logger, `local candidate queued: ${JSON.stringify(candidate)}`);
    const stored: RTCIceCandidateInit = {
      candidate: candidate.candidate,
      sdpMid: candidate.sdpMid ?? undefined,
      sdpMLineIndex: candidate.sdpMLineIndex ?? undefined,
      usernameFragment: candidate.usernameFragment ?? undefined,
    };
    pendingLocalCandidates.push(stored);
    allLocalCandidates.push(stored);
    if (candidateSendState === 'ready' && currentHandshakeId) {
      flushPendingCandidates();
    } else if (candidateSendState === 'delayed') {
      scheduleFlush(ANSWER_FLUSH_DELAY_MS);
    }
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
      localPeerId: assignedPeerId,
      afterSetRemoteDescription: () => {
        remoteDescriptionSet = true;
        // Drain any queued remote candidates now that the offer is applied.
        while (pendingRemoteCandidates.length > 0) {
          const cand = pendingRemoteCandidates.shift()!;
          pc
      .addIceCandidate(cand)
      .then(() => log(logger, `ice add ok: ${(cand.candidate ?? '').slice(0, 80)}`))
      .catch((error) => log(logger, `ice add failed: ${error}`));
        }
      },
      beforePostAnswer: () => {
        candidateSendState = 'delayed';
      },
      afterPostAnswer: () => {
        if (candidateSendState === 'delayed') {
          scheduleFlush(ANSWER_FLUSH_DELAY_MS);
        }
      },
    }, (handshakeId) => {
      currentHandshakeId = handshakeId;
      log(logger, `handshake ready: ${handshakeId}`);
      if (candidateSendState === 'ready') {
        flushPendingCandidates();
      }
    });
  } finally {
    disposeSignalListener();
    disposeGeneralListener();
  }
}

function attachSignalListener(
  signaling: SignalingClient,
  remotePeerId: string,
  getHandshakeId: () => string | null,
  onRemoteCandidate: (cand: RTCIceCandidateInit) => void,
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
    const handshakeId = getHandshakeId();
    if (
      handshakeId &&
      signal.signal.handshake_id &&
      signal.signal.handshake_id !== handshakeId
    ) {
      log(logger, `discarding signal for stale handshake ${signal.signal.handshake_id}`);
      return;
    }
    if (signal.signal.signal_type === 'ice_candidate') {
      const candidate: RTCIceCandidateInit = {
        candidate: signal.signal.candidate,
        sdpMid: signal.signal.sdp_mid ?? undefined,
        sdpMLineIndex: signal.signal.sdp_mline_index ?? undefined,
      };
      log(logger, 'received remote ice candidate');
      onRemoteCandidate(candidate);
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
  | {
      transport: 'webrtc';
      signal: { signal_type: 'offer' | 'answer'; sdp: string; handshake_id: string };
    }
  | {
      transport: 'webrtc';
      signal: {
        signal_type: 'ice_candidate';
        handshake_id: string;
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
    if (typeof signal.handshake_id !== 'string') {
      return undefined;
    }
    return {
      transport: 'webrtc',
      signal: {
        signal_type: signalType,
        sdp: typeof signal.sdp === 'string' ? signal.sdp : '',
        handshake_id: signal.handshake_id,
      },
    };
  }
  if (signalType === 'ice_candidate') {
    if (typeof signal.candidate !== 'string') {
      return undefined;
    }
    if (typeof signal.handshake_id !== 'string') {
      return undefined;
    }
    return {
      transport: 'webrtc',
      signal: {
        signal_type: 'ice_candidate',
        handshake_id: signal.handshake_id,
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
  afterSetRemoteDescription?: () => void;
  beforePostAnswer?: () => void;
  afterPostAnswer?: () => void;
  localPeerId: string;
}, onHandshakeReady: (handshakeId: string) => void): Promise<ConnectedWebRtcTransport> {
  const { pc, signalingUrl, pollIntervalMs, remotePeerId, logger, localPeerId } = options;
  log(logger, 'polling for SDP offer');
  const offer = await pollSdp(
    `${signalingUrl.replace(/\/$/, '')}/offer`,
    pollIntervalMs,
    { peer_id: localPeerId },
    logger,
  );
  log(logger, 'SDP offer received');
  const handshakeId = offer.handshake_id;
  if (!handshakeId) {
    throw new Error('offer missing handshake_id');
  }
  onHandshakeReady(handshakeId);
  const channelPromise = waitForDataChannel(pc, remotePeerId, logger);
  log(logger, 'waiting for data channel announcement');

  await pc.setRemoteDescription({ type: offer.type as RTCSdpType, sdp: offer.sdp });
  try {
    options.afterSetRemoteDescription?.();
  } catch {}

  const answer = await pc.createAnswer();
  await pc.setLocalDescription(answer);
  try {
    options.beforePostAnswer?.();
  } catch {}
  await postSdp(`${signalingUrl.replace(/\/$/, '')}/answer`, {
    sdp: answer.sdp ?? '',
    type: answer.type,
    handshake_id: handshakeId,
    from_peer: localPeerId,
    to_peer: offer.from_peer,
  });
  log(logger, 'SDP answer posted');
  try {
    options.afterPostAnswer?.();
  } catch {}

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
  params: Record<string, string> | undefined,
  logger?: (message: string) => void,
): Promise<WebRtcSdpPayload> {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const payload = await fetchSdp(url, params);
    if (payload) {
      log(logger, `polled SDP at ${url}`);
      return payload;
    }
    await delay(pollIntervalMs);
  }
  throw new Error('timed out waiting for SDP payload');
}

async function fetchSdp(
  url: string,
  params?: Record<string, string>,
): Promise<WebRtcSdpPayload | null> {
  const target = appendParams(url, params);
  const response = await fetch(target, { cache: 'no-cache' });
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

export function appendParams(
  url: string,
  params?: Record<string, string>,
): string {
  if (!params || Object.keys(params).length === 0) {
    return url;
  }
  const base = typeof window === 'undefined' ? 'http://localhost/' : window.location.href;
  const target = new URL(url, base);
  for (const [key, value] of Object.entries(params)) {
    target.searchParams.set(key, value);
  }
  return target.toString();
}

interface WebRtcSdpPayload {
  sdp: string;
  type: string;
  handshake_id: string;
  from_peer: string;
  to_peer: string;
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
