import { SignalingClient } from '../transport/signaling';
import type { SignalingClientOptions } from '../transport/signaling';
import { connectWebRtcTransport, type ConnectedWebRtcTransport } from '../transport/webrtc';

export interface ConnectUnifiedOptions {
  sessionId: string;
  baseUrl: string;
  passcode?: string;
  iceServers?: RTCIceServer[];
  logger?: (msg: string) => void;
  createSocket?: SignalingClientOptions['createSocket'];
  clientLabel?: string;
}

export interface UnifiedConnection {
  webrtc: ConnectedWebRtcTransport;
  signaling: SignalingClient;
  close(): void;
}

export async function connectUnified(options: ConnectUnifiedOptions): Promise<UnifiedConnection> {
  const join = await fetchJoinMetadataUnified(options);
  const websocketUrl = join.websocketUrl ?? deriveWebsocketUrl(options.baseUrl, options.sessionId);
  const signaling = await SignalingClient.connect({
    url: websocketUrl,
    passphrase: options.passcode,
    supportedTransports: ['webrtc'],
    createSocket: options.createSocket,
    label: options.clientLabel,
  });

  const webrtc = await connectWebRtcTransport({
    signaling,
    signalingUrl: join.signalingUrl,
    role: join.role,
    pollIntervalMs: join.pollIntervalMs,
    iceServers: options.iceServers,
    logger: options.logger,
    passphrase: options.passcode,
    telemetryBaseUrl: options.baseUrl,
    sessionId: options.sessionId,
  });

  return {
    webrtc,
    signaling,
    close: () => {
      try {
        webrtc.transport.close();
      } catch {}
      try {
        signaling.close();
      } catch {}
    },
  };
}

function deriveWebsocketUrl(baseUrl: string, sessionId: string): string {
  const trimmedBase = baseUrl.trim().replace(/\/$/, '');
  const normalised = normaliseBase(trimmedBase);
  normalised.pathname = `${normalised.pathname.replace(/\/$/, '')}/ws/${sessionId}`;
  normalised.search = '';
  normalised.hash = '';
  return normalised.toString();
}

function normaliseBase(input: string): URL {
  const hasScheme = /^https?:/i.test(input) || /^wss?:/i.test(input);
  const withScheme = hasScheme ? input : `https://${input}`;
  const url = new URL(withScheme);
  if (url.protocol === 'http:') {
    url.protocol = 'ws:';
  } else if (url.protocol === 'https:') {
    url.protocol = 'wss:';
  }
  return url;
}

async function fetchJoinMetadataUnified(options: ConnectUnifiedOptions): Promise<{
  signalingUrl: string;
  websocketUrl?: string;
  role: 'offerer' | 'answerer';
  pollIntervalMs: number;
}> {
  const url = `${options.baseUrl.replace(/\/$/, '')}/sessions/${options.sessionId}/join`;
  const response = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ passphrase: options.passcode ?? null }),
  });
  if (!response.ok) {
    throw new Error(`join request failed: ${response.status} ${response.statusText}`);
  }
  const payload: JoinSessionResponse = await response.json();
  if (!payload.success) {
    throw new Error(payload.message ?? 'join rejected');
  }
  const offerMetadata: OfferMetadata | undefined =
    payload.webrtc_offer ?? payload.transports?.find((t) => t.kind === 'webrtc')?.metadata;
  if (!offerMetadata) {
    throw new Error('webrtc offer metadata missing');
  }
  const signalingUrl: string | undefined =
    offerMetadata.signaling_url ?? offerMetadata.signalingUrl;
  if (!signalingUrl) {
    throw new Error('signaling_url missing from offer metadata');
  }
  const role = (offerMetadata.role ?? 'answerer') as 'offerer' | 'answerer';
  const pollIntervalMs =
    typeof offerMetadata.poll_interval_ms === 'number'
      ? offerMetadata.poll_interval_ms
      : typeof offerMetadata.pollIntervalMs === 'number'
        ? offerMetadata.pollIntervalMs
        : 250;

  return {
    signalingUrl,
    websocketUrl: payload.websocket_url ?? undefined,
    role,
    pollIntervalMs,
  };
}

interface OfferMetadata {
  signaling_url?: string;
  signalingUrl?: string;
  role?: 'offerer' | 'answerer';
  poll_interval_ms?: number;
  pollIntervalMs?: number;
  [key: string]: unknown;
}

interface JoinSessionResponse {
  success: boolean;
  message?: string;
  webrtc_offer?: OfferMetadata;
  transports?: Array<{ kind: string; metadata?: OfferMetadata }>;
  websocket_url?: string;
}

