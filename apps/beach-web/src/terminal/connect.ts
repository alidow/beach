import {
  DataChannelTerminalTransport,
  type TerminalTransport,
} from '../transport/terminalTransport';
import { connectWebRtcTransport } from '../transport/webrtc';
import { SignalingClient } from '../transport/signaling';
import type { SignalingClientOptions } from '../transport/signaling';

export interface BrowserTransportConnection {
  transport: TerminalTransport;
  signaling: SignalingClient;
  remotePeerId?: string;
  close(): void;
}

export interface ConnectBrowserTransportOptions {
  sessionId: string;
  baseUrl: string;
  passcode?: string;
  iceServers?: RTCIceServer[];
  logger?: (message: string) => void;
  createSocket?: SignalingClientOptions['createSocket'];
}

export async function connectBrowserTransport(
  options: ConnectBrowserTransportOptions,
): Promise<BrowserTransportConnection> {
  const join = await fetchJoinMetadata(options);
  const websocketUrl = join.websocketUrl ?? deriveWebsocketUrl(options.baseUrl, options.sessionId);
  const signaling = await SignalingClient.connect({
    url: websocketUrl,
    passphrase: options.passcode,
    supportedTransports: ['webrtc'],
    createSocket: options.createSocket,
  });
  const {
    transport: webRtcTransport,
    remotePeerId,
  } = await connectWebRtcTransport({
    signaling,
    signalingUrl: join.signalingUrl,
    role: join.role,
    pollIntervalMs: join.pollIntervalMs,
    iceServers: options.iceServers,
    logger: options.logger,
  });
  const transport = new DataChannelTerminalTransport(webRtcTransport, {
    logger: options.logger,
  });
  return {
    transport,
    signaling,
    remotePeerId,
    close: () => {
      transport.close();
      signaling.close();
    },
  };
}

export function deriveWebsocketUrl(baseUrl: string, sessionId: string): string {
  const trimmedBase = baseUrl.trim().replace(/\/$/, '');
  const normalised = normaliseBase(trimmedBase);
  normalised.pathname = `${normalised.pathname.replace(/\/$/, '')}/ws/${sessionId}`;
  normalised.search = '';
  normalised.hash = '';
  return normalised.toString();
}

async function fetchJoinMetadata(options: ConnectBrowserTransportOptions): Promise<{
  signalingUrl: string;
  websocketUrl?: string;
  role: 'offerer' | 'answerer';
  pollIntervalMs: number;
}> {
  const url = `${options.baseUrl.replace(/\/$/, '')}/sessions/${options.sessionId}/join`;
  const response = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ passphrase: options.passcode ?? null }),
  });
  if (!response.ok) {
    throw new Error(`join request failed: ${response.status} ${response.statusText}`);
  }
  const payload: JoinSessionResponse = await response.json();
  if (!payload.success) {
    throw new Error(payload.message ?? 'join rejected');
  }
  const offerMetadata =
    payload.webrtc_offer ??
    payload.transports?.find((transport) => transport.kind === 'webrtc')?.metadata;
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

interface JoinSessionResponse {
  success: boolean;
  message?: string;
  webrtc_offer?: any;
  transports?: Array<{ kind: string; metadata?: any }>;
  websocket_url?: string;
}
