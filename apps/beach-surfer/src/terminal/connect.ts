import {
  DataChannelTerminalTransport,
  type TerminalTransport,
} from '../transport/terminalTransport';
import { connectWebRtcTransport } from '../transport/webrtc';
import { SignalingClient } from '../transport/signaling';
import type { SignalingClientOptions } from '../transport/signaling';
import type { SecureTransportSummary } from '../transport/webrtc';
import type { ConnectionTrace } from '../lib/connectionTrace';

export interface FallbackOverrides {
  cohort?: string;
  entitlementProof?: string;
  telemetryOptIn?: boolean;
}

export interface BrowserTransportConnection {
  transport: TerminalTransport;
  signaling: SignalingClient;
  remotePeerId?: string;
  secure?: SecureTransportSummary;
  fallbackOverrides?: FallbackOverrides;
  close(): void;
}

export interface ConnectBrowserTransportOptions {
  sessionId: string;
  baseUrl: string;
  passcode?: string;
  viewerToken?: string | null;
  iceServers?: RTCIceServer[];
  logger?: (message: string) => void;
  createSocket?: SignalingClientOptions['createSocket'];
  clientLabel?: string;
  fallbackOverrides?: FallbackOverrides;
  trace?: ConnectionTrace | null;
  authorizationToken?: string;
}

const HOST_DOCKER_HOSTNAME = 'host.docker.internal';

function normalizeConnectorUrl(url: string | undefined): string | undefined {
  if (!url) return url;
  if (typeof window === 'undefined') return url;
  try {
    const parsed = new URL(url);
    if (parsed.hostname === HOST_DOCKER_HOSTNAME) {
      const replacementHost = window.location.hostname || 'localhost';
      parsed.hostname = replacementHost;
    }
    return parsed.toString();
  } catch {
    return url;
  }
}

export async function connectBrowserTransport(
  options: ConnectBrowserTransportOptions,
): Promise<BrowserTransportConnection> {
  const trace = options.trace ?? null;
  trace?.mark('connect_browser_transport:start', {
    hasPasscode: Boolean(options.passcode),
    hasViewerToken: Boolean(options.viewerToken),
  });
  if (typeof window !== 'undefined' && process.env.NODE_ENV !== 'production') {
    // eslint-disable-next-line no-console
    console.info('[connectBrowserTransport][rewrite] start', {
      sessionId: options.sessionId,
      baseUrl: options.baseUrl,
      hasPasscode: Boolean(options.passcode),
      hasViewerToken: Boolean(options.viewerToken),
      hasAuthorization: Boolean(options.authorizationToken && options.authorizationToken.trim().length > 0),
    });
  }

  const join = await fetchJoinMetadata(options);
  trace?.mark('join_metadata:received', {
    role: join.role,
    pollIntervalMs: join.pollIntervalMs,
    signalingUrl: join.signalingUrl,
  });
  if (typeof window !== 'undefined' && process.env.NODE_ENV !== 'production') {
    // eslint-disable-next-line no-console
    console.info('[connectBrowserTransport][rewrite] joinMetadata.received', {
      sessionId: options.sessionId,
      role: join.role,
      pollIntervalMs: join.pollIntervalMs,
      signalingUrl: join.signalingUrl,
    });
  }
  const normalizedSignalingUrl = normalizeConnectorUrl(join.signalingUrl) ?? join.signalingUrl;
  const websocketUrl =
    normalizeConnectorUrl(join.websocketUrl) ??
    deriveWebsocketUrl(options.baseUrl, options.sessionId);
  trace?.mark('signaling:connect_start', { websocketUrl });
  const signaling = await SignalingClient.connect({
    url: websocketUrl,
    passphrase: options.passcode,
    viewerToken: options.viewerToken ?? undefined,
    supportedTransports: ['webrtc'],
    createSocket: options.createSocket,
    label: options.clientLabel,
    trace,
  });
  trace?.mark('signaling:ready', { peerId: signaling.peerId });
  if (typeof window !== 'undefined' && process.env.NODE_ENV !== 'production') {
    // eslint-disable-next-line no-console
    console.info('[connectBrowserTransport][rewrite] signaling.connected', {
      sessionId: options.sessionId,
      peerId: signaling.peerId,
    });
  }
  let webrtcResult: Awaited<ReturnType<typeof connectWebRtcTransport>>;
  try {
    webrtcResult = await connectWebRtcTransport({
      signaling,
      signalingUrl: normalizedSignalingUrl,
      role: join.role,
      pollIntervalMs: join.pollIntervalMs,
      iceServers: options.iceServers,
      logger: options.logger,
      passphrase: options.passcode,
      viewerToken: options.viewerToken ?? undefined,
    telemetryBaseUrl: options.baseUrl,
    sessionId: options.sessionId,
    trace,
  });
  } catch (error) {
    trace?.mark('webrtc:connect_error', {
      message: error instanceof Error ? error.message : String(error),
    });
    if (typeof window !== 'undefined' && process.env.NODE_ENV !== 'production') {
      // eslint-disable-next-line no-console
      console.error('[connectBrowserTransport][rewrite] webrtc.connect_error', {
        sessionId: options.sessionId,
        message: error instanceof Error ? error.message : String(error),
      });
    }
    throw error;
  }
  const { transport: webRtcTransport, remotePeerId, secure } = webrtcResult;
  trace?.mark('webrtc:connected', {
    remotePeerId,
    secureMode: secure?.mode ?? 'plaintext',
  });
  if (typeof window !== 'undefined' && process.env.NODE_ENV !== 'production') {
    // eslint-disable-next-line no-console
    console.info('[connectBrowserTransport][rewrite] webrtc.connected', {
      sessionId: options.sessionId,
      remotePeerId,
      secureMode: secure?.mode ?? 'plaintext',
    });
  }
  const transport = new DataChannelTerminalTransport(webRtcTransport, {
    logger: options.logger,
    secureContext: secure,
  });
  if (options.fallbackOverrides && options.logger) {
    const { cohort, entitlementProof, telemetryOptIn } = options.fallbackOverrides;
    options.logger(
      `[fallback overrides] cohort=${cohort ?? '—'} proof=${entitlementProof ? 'present' : 'none'} telemetry=${
        telemetryOptIn ? 'on' : 'off'
      }`,
    );
  }
  return {
    transport,
    signaling,
    remotePeerId,
    secure,
    fallbackOverrides: options.fallbackOverrides,
    close: () => {
      transport.close();
      signaling.close();
      if (typeof window !== 'undefined' && process.env.NODE_ENV !== 'production') {
        // eslint-disable-next-line no-console
        console.info('[connectBrowserTransport][rewrite] connection.closed', {
          sessionId: options.sessionId,
        });
      }
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
  const trace = options.trace ?? null;
  const url = `${options.baseUrl.replace(/\/$/, '')}/sessions/${options.sessionId}/join`;
  trace?.mark('join_metadata:request', { url });
  if (typeof window !== 'undefined' && process.env.NODE_ENV !== 'production') {
    // eslint-disable-next-line no-console
    console.info('[connectBrowserTransport][rewrite] joinMetadata.request', {
      sessionId: options.sessionId,
      url,
    });
  }
  let response: Response;
  try {
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
    };
    if (options.authorizationToken && options.authorizationToken.trim().length > 0) {
      headers['Authorization'] = `Bearer ${options.authorizationToken.trim()}`;
    }
    const payload: Record<string, unknown> = {
      passphrase: options.passcode ?? null,
    };
    if (options.viewerToken && options.viewerToken.trim().length > 0) {
      payload.viewer_token = options.viewerToken.trim();
    }
    response = await fetch(url, {
      method: 'POST',
      headers,
      body: JSON.stringify(payload),
    });
  } catch (error) {
    trace?.mark('join_metadata:error', {
      message: error instanceof Error ? error.message : String(error),
    });
    throw error;
  }
  trace?.mark('join_metadata:response', { status: response.status });
  if (typeof window !== 'undefined' && process.env.NODE_ENV !== 'production') {
    // eslint-disable-next-line no-console
    console.info('[connectBrowserTransport][rewrite] joinMetadata.response', {
      sessionId: options.sessionId,
      status: response.status,
    });
  }
  if (!response.ok) {
    trace?.mark('join_metadata:failure', {
      status: response.status,
      statusText: response.statusText,
    });
    if (typeof window !== 'undefined' && process.env.NODE_ENV !== 'production') {
      // eslint-disable-next-line no-console
      console.error('[connectBrowserTransport][rewrite] joinMetadata.failure', {
        sessionId: options.sessionId,
        status: response.status,
        statusText: response.statusText,
      });
    }
    throw new Error(`join request failed: ${response.status} ${response.statusText}`);
  }
  const payload: JoinSessionResponse = await response.json();
  if (!payload.success) {
    trace?.mark('join_metadata:rejected', {
      message: payload.message ?? null,
    });
    throw new Error(payload.message ?? 'join rejected');
  }
  const offerMetadata: OfferMetadata | undefined =
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
