import { decodeTransportMessage, encodeTransportMessage, type TransportMessage } from './envelope';
import { deriveHandshakeKey, derivePreSharedKey, toHex } from './crypto/sharedKey';
import {
  secureSignalingEnabled,
  sealWithKey,
  openWithKey,
  type SealedEnvelope,
  type SignalingLabel,
} from './crypto/secureSignaling';
import {
  runBrowserHandshake,
  buildPrologueContext,
  type BrowserHandshakeResult,
  type BrowserHandshakeRole,
} from './crypto/noiseHandshake';
import {
  SecureDataChannel,
  type DataChannelLike,
} from './crypto/secureDataChannel';
import type { SignalingClient, ServerMessage } from './signaling';
import { reportSecureTransportEvent } from '../lib/telemetry';
import type { ConnectionTrace } from '../lib/connectionTrace';

export interface SecureTransportSummary {
  mode: 'secure' | 'plaintext';
  verificationCode?: string;
  handshakeId?: string;
  remotePeerId?: string;
}

export type WebRtcTransportPayload = TransportMessage['payload'];

export type WebRtcTransportEventMap = {
  message: CustomEvent<TransportMessage>;
  open: Event;
  close: Event;
  error: Event;
  secure: CustomEvent<SecureTransportSummary>;
};

export interface WebRtcTransportOptions {
  channel: DataChannelLike;
  /** Optional initial outbound sequence value. Useful for deterministic tests. */
  initialSequence?: number;
  secureSummary?: SecureTransportSummary;
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
  private readonly secureSummary: SecureTransportSummary;

  constructor(options: WebRtcTransportOptions) {
    super();
    this.channel = options.channel;
    this.channel.binaryType = 'arraybuffer';
    this.sequence = options.initialSequence ?? 0;
    this.open = this.channel.readyState === 'open';
    this.secureSummary = options.secureSummary ?? { mode: 'plaintext' };
    this.attachChannelListeners();
    queueMicrotask(() => {
      this.dispatchEvent(new CustomEvent<SecureTransportSummary>('secure', { detail: this.secureSummary }));
    });
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

  getSecureSummary(): SecureTransportSummary {
    return this.secureSummary;
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
  passphrase?: string;
  telemetryBaseUrl?: string;
  sessionId?: string;
  trace?: ConnectionTrace | null;
}

export interface ConnectedWebRtcTransport {
  transport: WebRtcTransport;
  peerConnection: RTCPeerConnection;
  dataChannel: RTCDataChannel;
  remotePeerId: string;
  secure?: SecureTransportSummary;
}

const ANSWER_FLUSH_DELAY_MS = 400;
const HANDSHAKE_CHANNEL_LABEL = 'beach-secure-handshake';

interface HandshakeOptions {
  role: BrowserHandshakeRole;
  handshakeId: string;
  localPeerId: string;
  remotePeerId: string;
  prologueContext: Uint8Array;
  keyPromise: Promise<Uint8Array>;
  telemetryBaseUrl?: string;
  sessionId?: string;
}

export async function connectWebRtcTransport(
  options: ConnectWebRtcTransportOptions,
): Promise<ConnectedWebRtcTransport> {
  const { signaling, logger } = options;
  const trace = options.trace ?? null;
  trace?.mark('webrtc:connect_start', { role: options.role });
  const passphrase = options.passphrase?.trim();
  const secureSignalingActive = Boolean(
    secureSignalingEnabled() && passphrase && passphrase.length > 0,
  );
  let sessionKeyPromise: Promise<Uint8Array> | null = null;
  const handshakeKeyPromises = new Map<string, Promise<Uint8Array>>();
  let currentHandshakeId: string | null = null;
  const ensureSealingKey = (handshakeId: string): Promise<Uint8Array> => {
    const existing = handshakeKeyPromises.get(handshakeId);
    if (existing) {
      return existing;
    }
    if (!secureSignalingActive) {
      throw new Error('secure signaling disabled');
    }
    if (!passphrase) {
      throw new Error('secure signaling requires passphrase');
    }
    if (!sessionKeyPromise) {
      throw new Error('session key not ready');
    }
    trace?.mark('webrtc:derive_handshake_key_start', { handshakeId });
    const promise = sessionKeyPromise
      .then((sessionKey) => deriveHandshakeKey(sessionKey, handshakeId))
      .then((value) => {
        trace?.mark('webrtc:derive_handshake_key_complete', { handshakeId });
        return value;
      })
      .catch((error) => {
        trace?.mark('webrtc:derive_handshake_key_error', {
          handshakeId,
          message: String(error),
        });
        handshakeKeyPromises.delete(handshakeId);
        throw error;
      });
    handshakeKeyPromises.set(handshakeId, promise);
    return promise;
  };
  const getSealingKey = () =>
    currentHandshakeId ? handshakeKeyPromises.get(currentHandshakeId) ?? null : null;
  const join = await signaling.waitForMessage('join_success', 15_000);
  trace?.mark('signaling:join_success', {
    assignedPeerId: join.peer_id,
    peers: join.peers.length,
  });
  log(logger, `join_success payload: ${JSON.stringify(join)}`);
  const sessionId = options.sessionId ?? join.session_id;
  if (secureSignalingActive) {
    trace?.mark('webrtc:derive_session_key_start', { sessionId });
    sessionKeyPromise = derivePreSharedKey(passphrase!, sessionId)
      .then((value) => {
        trace?.mark('webrtc:derive_session_key_complete', { sessionId });
        return value;
      })
      .catch((error) => {
        trace?.mark('webrtc:derive_session_key_error', {
          sessionId,
          message: String(error),
        });
        sessionKeyPromise = null;
        throw error;
      });
  }
  const assignedPeerId = join.peer_id;
  const remotePeerId = await resolveRemotePeerId(signaling, join, options.preferredPeerId);
  trace?.mark('webrtc:remote_peer_resolved', { remotePeerId });
  log(logger, `remote peer resolved: ${remotePeerId}`);

  const secureState = {
    enabled: secureSignalingActive,
    localPeerId: assignedPeerId,
    ensureKey: ensureSealingKey,
    getKey: getSealingKey,
  } as const;

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
  const disposeSignalListener = attachSignalListener(
    signaling,
    remotePeerId,
    () => currentHandshakeId,
    onRemoteCandidate,
    secureState,
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

  const dispatchCandidate = async (candidate: RTCIceCandidateInit) => {
    if (!currentHandshakeId) {
      pendingLocalCandidates.unshift(candidate);
      return;
    }
    try {
      log(logger, `sending local candidate: ${JSON.stringify(candidate)}`);
      const signalPayload: any = {
        transport: 'webrtc',
        signal: {
          signal_type: 'ice_candidate',
          handshake_id: currentHandshakeId,
          candidate: candidate.candidate ?? '',
          sdp_mid: candidate.sdpMid ?? undefined,
          sdp_mline_index: candidate.sdpMLineIndex ?? undefined,
        },
      };

      if (secureState.enabled) {
        const key = await secureState.ensureKey(currentHandshakeId);
        const envelope = await sealIceCandidate({
          key,
          handshakeId: currentHandshakeId,
          localPeerId: secureState.localPeerId,
          remotePeerId,
          candidate,
        });
        signalPayload.signal.candidate = '';
        delete signalPayload.signal.sdp_mid;
        delete signalPayload.signal.sdp_mline_index;
        signalPayload.signal.sealed = envelope;
      }

      signaling.send({
        type: 'signal',
        to_peer: remotePeerId,
        signal: signalPayload,
      });
    } catch (error) {
      log(logger, `failed to send ICE candidate: ${String(error)}`);
    }
  };

  const flushPendingCandidates = async () => {
    if (pendingLocalCandidates.length === 0 || !currentHandshakeId) {
      return;
    }
    while (pendingLocalCandidates.length > 0) {
      const candidate = pendingLocalCandidates.shift()!;
      try {
        await dispatchCandidate(candidate);
      } catch (error) {
        log(logger, `failed to flush candidate: ${String(error)}`);
      }
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
      void flushPendingCandidates();
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
      void (async () => {
        for (const candidate of allLocalCandidates) {
          try {
            await dispatchCandidate({ ...candidate });
          } catch (error) {
            log(logger, `failed to resend candidate: ${String(error)}`);
          }
        }
        if (resendAttempts < MAX_RESEND_ATTEMPTS) {
          scheduleResend();
        }
      })();
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
      void flushPendingCandidates();
    } else if (candidateSendState === 'delayed') {
      scheduleFlush(ANSWER_FLUSH_DELAY_MS);
    }
  };

  try {
    if (options.role !== 'answerer') {
      throw new Error(`webrtc role ${options.role} not supported in browser client yet`);
    }
    const connected = await connectAsAnswerer({
      pc,
      signaling,
      signalingUrl: options.signalingUrl,
      pollIntervalMs: options.pollIntervalMs,
      remotePeerId,
      logger,
      localPeerId: assignedPeerId,
      secure: {
        enabled: secureState.enabled,
        passphrase,
        ensureKey: ensureSealingKey,
        getKey: getSealingKey,
        localPeerId: assignedPeerId,
        remotePeerId,
      },
      sessionId,
      trace,
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
      trace?.mark('webrtc:handshake_ready', { handshakeId });
      if (secureState.enabled) {
        void ensureSealingKey(handshakeId);
      }
      if (candidateSendState === 'ready') {
        void flushPendingCandidates();
      }
    });
    trace?.mark('webrtc:connect_complete', {
      remotePeerId: connected.remotePeerId,
      dataChannelLabel: connected.dataChannel.label,
      secureMode: connected.secure?.mode ?? 'plaintext',
    });
    return connected;
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
  secure: {
    enabled: boolean;
    getKey: () => Promise<Uint8Array> | null;
    localPeerId: string;
  },
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
      const candidateSignal = signal.signal;
      void (async () => {
        const handshakeId = getHandshakeId();
        if (!handshakeId) {
          log(logger, 'ignoring remote candidate: handshake not established');
          return;
        }

        let candidateInit: RTCIceCandidateInit;
        if (secure.enabled) {
          const keyPromise = secure.getKey();
          if (!keyPromise) {
            log(logger, 'ignoring remote candidate: secure key not ready');
            return;
          }
          const sealed = candidateSignal.sealed;
          if (!sealed) {
            log(logger, 'ignoring remote candidate: sealed payload missing');
            return;
          }
          try {
            const key = await keyPromise;
            const decoded = await openIceCandidate({
              key,
              handshakeId,
              localPeerId: secure.localPeerId,
              remotePeerId,
              envelope: sealed,
            });
            candidateInit = decoded;
          } catch (error) {
            log(logger, `failed to decrypt remote candidate: ${String(error)}`);
            return;
          }
        } else {
          candidateInit = {
            candidate: candidateSignal.candidate,
            sdpMid: candidateSignal.sdp_mid ?? undefined,
            sdpMLineIndex: candidateSignal.sdp_mline_index ?? undefined,
          };
        }

        log(logger, 'received remote ice candidate');
        onRemoteCandidate(candidateInit);
      })();
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
      signal: {
        signal_type: 'offer' | 'answer';
        sdp: string;
        handshake_id: string;
        sealed?: SealedEnvelope;
      };
    }
  | {
      transport: 'webrtc';
      signal: {
        signal_type: 'ice_candidate';
        handshake_id: string;
        candidate: string;
        sdp_mid?: string;
        sdp_mline_index?: number;
        sealed?: SealedEnvelope;
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
        sealed: parseSealed(signal.sealed),
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
        sealed: parseSealed(signal.sealed),
      },
    };
  }
  return undefined;
}

function parseSealed(value: unknown): SealedEnvelope | undefined {
  if (!value || typeof value !== 'object') {
    return undefined;
  }
  const version = (value as any).version;
  const nonce = (value as any).nonce;
  const ciphertext = (value as any).ciphertext;
  if (
    typeof version === 'number' &&
    typeof nonce === 'string' &&
    typeof ciphertext === 'string'
  ) {
    return { version, nonce, ciphertext };
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
  telemetryBaseUrl?: string;
  sessionId?: string;
  trace?: ConnectionTrace | null;
  secure: {
    enabled: boolean;
    passphrase?: string;
    ensureKey: (handshakeId: string) => Promise<Uint8Array>;
    getKey: () => Promise<Uint8Array> | null;
    localPeerId: string;
    remotePeerId: string;
  };
}, onHandshakeReady: (handshakeId: string) => void): Promise<ConnectedWebRtcTransport> {
  const {
    pc,
    signalingUrl,
    pollIntervalMs,
    remotePeerId,
    logger,
    localPeerId,
    secure,
    telemetryBaseUrl,
    sessionId,
    trace,
  } = options;
  let cachedSecureKey: Uint8Array | null = null;
  trace?.mark('webrtc:offer_poll_start', {
    url: `${signalingUrl.replace(/\/$/, '')}/offer`,
    remotePeerId,
  });
  log(logger, 'polling for SDP offer');
  const offer = await pollSdp(
    `${signalingUrl.replace(/\/$/, '')}/offer`,
    pollIntervalMs,
    { peer_id: localPeerId },
    logger,
  );
  trace?.mark('webrtc:offer_received', { handshakeId: offer.handshake_id, sealed: Boolean(offer.sealed) });
  log(logger, 'SDP offer received');
  if (offer.sealed) {
    log(logger, `offer sealed envelope ${JSON.stringify(offer.sealed)}`);
  } else {
    log(logger, 'offer not sealed (plaintext)');
  }
  const handshakeId = offer.handshake_id;
  if (!handshakeId) {
    throw new Error('offer missing handshake_id');
  }
  const associatedData = [offer.from_peer, offer.to_peer, offer.type];
  log(logger, `offer associated data ${JSON.stringify(associatedData)}`);
  if (secure.enabled) {
    if (!secure.passphrase) {
      throw new Error('secure signaling requires passphrase');
    }
    if (!offer.sealed) {
      throw new Error('expected sealed offer payload');
    }
    const key = await secure.ensureKey(handshakeId);
    log(logger, `derived handshake key for ${handshakeId}: ${toHex(key)}`);
    cachedSecureKey = key;
    try {
      const plaintext = await openSdpWithKey({
        key,
        handshakeId,
        label: 'offer',
        payload: offer,
      });
      offer.sdp = plaintext;
    } catch (error) {
      log(logger, `offer decrypt failed for handshake ${handshakeId}: ${String(error)}`);
      throw error;
    }
  }
  onHandshakeReady(handshakeId);
  const prologueContext = buildPrologueContext(
    handshakeId,
    secure.localPeerId,
    secure.remotePeerId,
  );
  const handshakeKeyPromise =
    secure.enabled && cachedSecureKey
      ? Promise.resolve(cachedSecureKey)
      : secure.enabled
        ? secure.ensureKey(handshakeId)
        : undefined;
  const channelPromise = waitForDataChannel(pc, {
    remotePeerId,
    logger,
    handshake: secure.enabled
      ? {
          role: 'responder',
          handshakeId,
          localPeerId: secure.localPeerId,
          remotePeerId: secure.remotePeerId,
          prologueContext,
          keyPromise: handshakeKeyPromise!,
          telemetryBaseUrl,
          sessionId,
        }
      : undefined,
    telemetryBaseUrl,
    sessionId,
    trace,
  });
  log(logger, 'waiting for data channel announcement');

  await pc.setRemoteDescription({ type: offer.type as RTCSdpType, sdp: offer.sdp });
  trace?.mark('webrtc:set_remote_description', { handshakeId });
  try {
    options.afterSetRemoteDescription?.();
  } catch {}

  const answer = await pc.createAnswer();
  await pc.setLocalDescription(answer);
  trace?.mark('webrtc:set_local_description', { handshakeId });
  try {
    options.beforePostAnswer?.();
  } catch {}
  const answerPayload: WebRtcSdpPayload = {
    sdp: answer.sdp ?? '',
    type: answer.type,
    handshake_id: handshakeId,
    from_peer: localPeerId,
    to_peer: offer.from_peer,
  };

  if (secure.enabled) {
    const key =
      cachedSecureKey ??
      (await secure.ensureKey(handshakeId));
    cachedSecureKey = key;
    const sealed = await sealSdpWithKey({
      key,
      handshakeId,
      label: 'answer',
      payload: answerPayload,
    });
    answerPayload.sdp = '';
    answerPayload.sealed = sealed;
  }

  await postSdp(`${signalingUrl.replace(/\/$/, '')}/answer`, answerPayload);
  trace?.mark('webrtc:answer_posted', { handshakeId });
  log(logger, 'SDP answer posted');
  try {
    options.afterPostAnswer?.();
  } catch {}

  return await channelPromise;
}

async function waitForDataChannel(
  pc: RTCPeerConnection,
  options: {
    remotePeerId: string;
    logger?: (message: string) => void;
    handshake?: HandshakeOptions;
    telemetryBaseUrl?: string;
    sessionId?: string;
    trace?: ConnectionTrace | null;
  },
): Promise<ConnectedWebRtcTransport> {
  const trace = options.trace ?? null;
  return await new Promise<ConnectedWebRtcTransport>((resolve, reject) => {
    let cleaned = false;
    const cleanupCallbacks: Array<() => void> = [];
    const cleanup = () => {
      if (cleaned) {
        return;
      }
      cleaned = true;
      clearTimeout(timeout);
      pc.removeEventListener('datachannel', handleDataChannel);
      while (cleanupCallbacks.length > 0) {
        const fn = cleanupCallbacks.pop()!;
        try {
          fn();
        } catch {}
      }
    };

    const timeout = setTimeout(() => {
      cleanup();
      trace?.mark('webrtc:data_channel_timeout');
      reject(new Error('timed out waiting for data channel'));
    }, 20_000);

    let resolveHandshake: ((value: BrowserHandshakeResult) => void) | null = null;
    let rejectHandshake: ((reason?: unknown) => void) | null = null;
    const handshakePromise: Promise<BrowserHandshakeResult | null> = options.handshake
      ? new Promise<BrowserHandshakeResult>((res, rej) => {
          resolveHandshake = res;
          rejectHandshake = rej;
        })
      : Promise.resolve(null);

    const handleDataChannel = (event: RTCDataChannelEvent) => {
      const channel = event.channel;
      log(options.logger, `data channel announced: ${channel.label}`);
      trace?.mark('webrtc:data_channel_announced', {
        label: channel.label,
        readyState: channel.readyState,
      });

      if (channel.label === HANDSHAKE_CHANNEL_LABEL) {
        if (!options.handshake) {
          log(options.logger, 'unexpected handshake channel; closing');
          try {
            channel.close();
          } catch {}
          return;
        }
        const cfg = options.handshake;
        const startedAt = typeof performance !== 'undefined' ? performance.now() : Date.now();
        trace?.mark('webrtc:noise_handshake_start', {
          handshakeId: cfg.handshakeId,
          role: cfg.role,
        });
        runBrowserHandshake(channel, {
          role: cfg.role,
          handshakeId: cfg.handshakeId,
          localPeerId: cfg.localPeerId,
          remotePeerId: cfg.remotePeerId,
          prologueContext: cfg.prologueContext,
          preSharedKeyPromise: cfg.keyPromise,
        })
          .then((result) => {
            log(
              options.logger,
              `secure handshake complete; verification ${result.verificationCode}`,
            );
            const finishedAt = typeof performance !== 'undefined' ? performance.now() : Date.now();
            trace?.mark('webrtc:noise_handshake_complete', {
              handshakeId: cfg.handshakeId,
              verificationCode: result.verificationCode,
              latencyMs: Math.max(0, finishedAt - startedAt),
            });
            void reportSecureTransportEvent(cfg.telemetryBaseUrl ?? options.telemetryBaseUrl, {
              sessionId: cfg.sessionId ?? options.sessionId,
              handshakeId: cfg.handshakeId,
              role: cfg.role === 'initiator' ? 'offerer' : 'answerer',
              outcome: 'success',
              verificationCode: result.verificationCode,
              latencyMs: Math.max(0, finishedAt - startedAt),
            });
            resolveHandshake?.(result);
            try {
              channel.close();
            } catch {}
          })
          .catch((error) => {
            trace?.mark('webrtc:noise_handshake_error', {
              handshakeId: cfg.handshakeId,
              message: error instanceof Error ? error.message : String(error),
            });
            void reportSecureTransportEvent(cfg.telemetryBaseUrl ?? options.telemetryBaseUrl, {
              sessionId: cfg.sessionId ?? options.sessionId,
              handshakeId: cfg.handshakeId,
              role: cfg.role === 'initiator' ? 'offerer' : 'answerer',
              outcome: 'failure',
              reason: error instanceof Error ? error.message : String(error),
            });
            rejectHandshake?.(error);
            cleanup();
            reject(error instanceof Error ? error : new Error(String(error)));
          });
        return;
      }

      const prepare = async () => {
        try {
          trace?.mark('webrtc:data_channel_prepare', {
            label: channel.label,
          });
          const handshakeResult = await handshakePromise;
          const secureSummary: SecureTransportSummary =
            handshakeResult && options.handshake
              ? {
                  mode: 'secure',
                  verificationCode: handshakeResult.verificationCode,
                  handshakeId: options.handshake.handshakeId,
                  remotePeerId: options.handshake.remotePeerId,
                }
              : { mode: 'plaintext' };
          const wrappedChannel: DataChannelLike =
            handshakeResult !== null
              ? new SecureDataChannel(channel, {
                  sendKey: handshakeResult.sendKey,
                  recvKey: handshakeResult.recvKey,
                })
              : channel;
          trace?.mark('webrtc:data_channel_secure_ready', {
            label: channel.label,
            secureMode: secureSummary.mode,
          });

          const transport = new WebRtcTransport({
            channel: wrappedChannel,
            secureSummary,
          });

          const handleOpen = () => {
            cleanup();
            log(options.logger, 'data channel open');
            trace?.mark('webrtc:data_channel_open', {
              label: channel.label,
              readyState: channel.readyState,
            });
            resolve({
              transport,
              peerConnection: pc,
              dataChannel: channel,
              remotePeerId: options.remotePeerId,
              secure: secureSummary,
            });
          };

          const handleError = (event: Event) => {
            cleanup();
            const errorInstance = (event as any).error ?? new Error('data channel error');
            trace?.mark('webrtc:data_channel_error', {
              message: errorInstance instanceof Error ? errorInstance.message : String(errorInstance),
            });
            reject(errorInstance);
          };

          cleanupCallbacks.push(() => {
            wrappedChannel.removeEventListener('open', handleOpen);
            wrappedChannel.removeEventListener('error', handleError);
          });

          wrappedChannel.addEventListener('open', handleOpen, { once: true });
          wrappedChannel.addEventListener('error', handleError, { once: true });
          wrappedChannel.addEventListener('close', () =>
            {
              log(options.logger, 'data channel closed');
              trace?.mark('webrtc:data_channel_close', { label: channel.label });
            },
          );
        } catch (error) {
          cleanup();
          reject(error instanceof Error ? error : new Error(String(error)));
        }
      };

      void prepare();
    };

    pc.addEventListener('datachannel', handleDataChannel);
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

async function sealSdpWithKey(options: {
  key: Uint8Array;
  handshakeId: string;
  label: 'offer' | 'answer';
  payload: WebRtcSdpPayload;
}): Promise<SealedEnvelope> {
  const { key, handshakeId, label, payload } = options;
  return await sealWithKey({
    psk: key,
    handshakeId,
    label,
    associatedData: [payload.from_peer, payload.to_peer, payload.type],
    plaintext: payload.sdp,
  });
}

async function openSdpWithKey(options: {
  key: Uint8Array;
  handshakeId: string;
  label: 'offer' | 'answer';
  payload: WebRtcSdpPayload;
}): Promise<string> {
  const { key, handshakeId, label, payload } = options;
  if (!payload.sealed) {
    throw new Error('sealed envelope missing');
  }
  return await openWithKey({
    psk: key,
    handshakeId,
    label,
    associatedData: [payload.from_peer, payload.to_peer, payload.type],
    envelope: payload.sealed,
  });
}

async function sealIceCandidate(options: {
  key: Uint8Array;
  handshakeId: string;
  localPeerId: string;
  remotePeerId: string;
  candidate: RTCIceCandidateInit;
}): Promise<SealedEnvelope> {
  const { key, handshakeId, localPeerId, remotePeerId, candidate } = options;
  const plaintext = JSON.stringify({
    candidate: candidate.candidate ?? '',
    sdp_mid: candidate.sdpMid ?? null,
    sdp_mline_index: candidate.sdpMLineIndex ?? null,
  });
  return await sealWithKey({
    psk: key,
    handshakeId,
    label: 'ice',
    associatedData: [localPeerId, remotePeerId, handshakeId],
    plaintext,
  });
}

async function openIceCandidate(options: {
  key: Uint8Array;
  handshakeId: string;
  localPeerId: string;
  remotePeerId: string;
  envelope: SealedEnvelope;
}): Promise<RTCIceCandidateInit> {
  const { key, handshakeId, localPeerId, remotePeerId, envelope } = options;
  const plaintext = await openWithKey({
    psk: key,
    handshakeId,
    label: 'ice',
    associatedData: [remotePeerId, localPeerId, handshakeId],
    envelope,
  });
  let parsed: any;
  try {
    parsed = JSON.parse(plaintext);
  } catch (error) {
    throw new Error(`failed to parse sealed ICE candidate: ${String(error)}`);
  }
  return {
    candidate: typeof parsed.candidate === 'string' ? parsed.candidate : '',
    sdpMid: typeof parsed.sdp_mid === 'string' ? parsed.sdp_mid : undefined,
    sdpMLineIndex:
      typeof parsed.sdp_mline_index === 'number' ? parsed.sdp_mline_index : undefined,
  };
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
  sealed?: SealedEnvelope;
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
