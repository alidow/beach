import {
  DataChannelTerminalTransport,
  type TerminalTransport,
} from '../transport/terminalTransport';
import { attachPeerSession, connectWebRtcTransport } from '../transport/webrtc';
import { SignalingClient, generatePeerId } from '../transport/signaling';
import type { SignalingClientOptions } from '../transport/signaling';
import type { SecureTransportSummary } from '../transport/webrtc';
import type { ConnectionTrace } from '../lib/connectionTrace';
import { maybeParseIceServers } from '../transport/webrtcIceConfig';

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
  iceServersExpiresAtMs?: number;
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
  onIceRefresh?: (context: IceRefreshContext) => void | boolean | Promise<void | boolean>;
}

const HOST_DOCKER_HOSTNAME = 'host.docker.internal';
const sessionLocks = new Map<string, Promise<void>>();

type IceRefreshReason = 'scheduled' | 'retry';

type JoinMetadata = {
  signalingUrl: string;
  websocketUrl?: string;
  role: 'offerer' | 'answerer';
  pollIntervalMs: number;
  iceServers?: RTCIceServer[];
  iceServersExpiresAtMs?: number;
  raw: JoinSessionResponse;
};

export type IceRefreshContext = {
  join: JoinSessionResponse;
  iceServers: RTCIceServer[];
  expiresAtMs?: number;
  reason: IceRefreshReason;
};

async function acquireSessionLock(sessionId: string): Promise<() => void> {
  const previous = sessionLocks.get(sessionId) ?? Promise.resolve();
  let resolveCurrent: () => void = () => {};
  const currentReady = new Promise<void>((resolve) => {
    resolveCurrent = resolve;
  });
  const chained = previous.then(() => currentReady);
  sessionLocks.set(sessionId, chained);
  await previous;
  return () => {
    resolveCurrent();
    if (sessionLocks.get(sessionId) === chained) {
      sessionLocks.delete(sessionId);
    }
  };
}

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
  const releaseLock = await acquireSessionLock(options.sessionId);
  const trace = options.trace ?? null;
  let refreshTimer: ReturnType<typeof setTimeout> | null = null;
  let closed = false;
  let signaling: SignalingClient | null = null;
  let terminalTransport: DataChannelTerminalTransport | null = null;

  const clearRefreshTimer = () => {
    if (refreshTimer) {
      clearTimeout(refreshTimer);
      refreshTimer = null;
    }
  };

  const closeConnection = (reason: string) => {
    if (closed) {
      return;
    }
    closed = true;
    clearRefreshTimer();
    try {
      terminalTransport?.close();
    } catch {
      // ignore
    }
    try {
      signaling?.close();
    } catch {
      // ignore
    }
    if (typeof window !== 'undefined' && process.env.NODE_ENV !== 'production') {
      // eslint-disable-next-line no-console
      console.info('[connectBrowserTransport][rewrite] connection.closed', {
        sessionId: options.sessionId,
        reason,
      });
    }
  };

  const maybeHandleIceRefresh = async (
    join: JoinMetadata,
    iceServers: RTCIceServer[],
    reason: IceRefreshReason,
  ): Promise<boolean> => {
    if (!options.onIceRefresh) {
      return false;
    }
    try {
      const result = await options.onIceRefresh({
        join: join.raw,
        iceServers,
        expiresAtMs: join.iceServersExpiresAtMs,
        reason,
      });
      return result === true;
    } catch (error) {
      trace?.mark('ice_refresh:callback_error', {
        message: error instanceof Error ? error.message : String(error),
      });
      if (typeof window !== 'undefined' && process.env.NODE_ENV !== 'production') {
        // eslint-disable-next-line no-console
        console.error('[connectBrowserTransport][rewrite] iceRefresh.callback_error', {
          sessionId: options.sessionId,
          message: error instanceof Error ? error.message : String(error),
        });
      }
      return false;
    }
  };

  const scheduleIceRefresh = (expiresAtMs?: number) => {
    if (closed) {
      return;
    }
    const delay = computeIceRefreshDelay(expiresAtMs);
    if (delay === null) {
      return;
    }
    refreshTimer = setTimeout(() => {
      void refreshIceServers('scheduled');
    }, delay);
  };

  const scheduleRefreshRetry = () => {
    if (closed) {
      return;
    }
    refreshTimer = setTimeout(() => {
      void refreshIceServers('retry');
    }, 30_000);
  };

  const refreshIceServers = async (reason: IceRefreshReason) => {
    if (closed) {
      return;
    }
    clearRefreshTimer();
    trace?.mark('ice_refresh:start', { sessionId: options.sessionId, reason });
    try {
      const refreshed = await fetchJoinMetadata(options);
      const refreshedIce = selectIceServers(refreshed, options);
      if (!refreshedIce || refreshedIce.length === 0) {
        trace?.mark('ice_refresh:empty', { sessionId: options.sessionId });
        scheduleRefreshRetry();
        return;
      }
      const handled = await maybeHandleIceRefresh(refreshed, refreshedIce, reason);
      if (!handled && !closed) {
        closeConnection('ice_refresh');
        return;
      }
      if (!closed) {
        scheduleIceRefresh(refreshed.iceServersExpiresAtMs);
      }
    } catch (error) {
      trace?.mark('ice_refresh:error', {
        sessionId: options.sessionId,
        message: error instanceof Error ? error.message : String(error),
      });
      if (typeof window !== 'undefined' && process.env.NODE_ENV !== 'production') {
        // eslint-disable-next-line no-console
        console.warn('[connectBrowserTransport][rewrite] iceRefresh.error', {
          sessionId: options.sessionId,
          message: error instanceof Error ? error.message : String(error),
        });
      }
      scheduleRefreshRetry();
    }
  };

  try {
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
        hasAuthorization: Boolean(
          options.authorizationToken && options.authorizationToken.trim().length > 0,
        ),
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
    const peerId = generatePeerId();
    const attached = await attachPeerSession({
      signalingUrl: normalizedSignalingUrl,
      role: join.role,
      peerId,
      passphrase: options.passcode ?? undefined,
    });
    const websocketUrl =
      normalizeConnectorUrl(join.websocketUrl) ?? attached.websocketUrl ?? deriveWebsocketUrl(options.baseUrl, options.sessionId);
    trace?.mark('signaling:connect_start', { websocketUrl });
    signaling = await SignalingClient.connect({
      url: websocketUrl,
      peerId,
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
    const iceServers = selectIceServers(join, options);
    try {
      webrtcResult = await connectWebRtcTransport({
        signaling,
        signalingUrl: normalizedSignalingUrl,
        role: join.role,
        pollIntervalMs: join.pollIntervalMs,
        iceServers,
        logger: options.logger,
        passphrase: options.passcode,
        viewerToken: options.viewerToken ?? undefined,
        telemetryBaseUrl: options.baseUrl,
        sessionId: attached.hostSessionId ?? options.sessionId,
        signalingUrl: attached.signalingUrl,
        peerSessionId: attached.peerSessionId,
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
    terminalTransport = transport;
    scheduleIceRefresh(join.iceServersExpiresAtMs);
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
      iceServersExpiresAtMs: join.iceServersExpiresAtMs,
      close: () => closeConnection('caller'),
    };
  } finally {
    releaseLock();
  }
}

export function deriveWebsocketUrl(baseUrl: string, sessionId: string): string {
  const trimmedBase = baseUrl.trim().replace(/\/$/, '');
  const normalised = normaliseBase(trimmedBase);
  normalised.pathname = `${normalised.pathname.replace(/\/$/, '')}/ws/${sessionId}`;
  normalised.search = '';
  normalised.hash = '';
  return normalised.toString();
}

function computeIceRefreshDelay(expiresAtMs?: number): number | null {
  if (typeof expiresAtMs !== 'number') {
    return null;
  }
  const ttlMs = expiresAtMs - Date.now();
  if (ttlMs <= 0) {
    return null;
  }
  const delay = Math.floor(ttlMs * 0.8);
  return delay > 0 ? delay : null;
}

function normalizeIceServers(raw?: unknown): RTCIceServer[] | undefined {
  if (!Array.isArray(raw)) {
    return undefined;
  }
  const normalized = raw
    .map((server) => {
      if (!server || typeof server !== 'object') {
        return null;
      }
      const urlsRaw = (server as any).urls;
      const urls =
        typeof urlsRaw === 'string'
          ? urlsRaw.split(',').map((u) => u.trim()).filter(Boolean)
          : Array.isArray(urlsRaw)
            ? urlsRaw.map((u) => (typeof u === 'string' ? u.trim() : '')).filter(Boolean)
            : [];
      if (urls.length === 0) {
        return null;
      }
      const candidate: RTCIceServer = { urls };
      if (typeof (server as any).username === 'string' && (server as any).username.trim().length > 0) {
        candidate.username = (server as any).username.trim();
      }
      if (
        typeof (server as any).credential === 'string' &&
        (server as any).credential.trim().length > 0
      ) {
        candidate.credential = (server as any).credential.trim();
      }
      return candidate;
    })
    .filter((server): server is RTCIceServer => Boolean(server));
  return normalized.length > 0 ? normalized : undefined;
}

function selectIceServers(
  join: JoinMetadata,
  options: ConnectBrowserTransportOptions,
): RTCIceServer[] | undefined {
  if (join.iceServers !== undefined) {
    return join.iceServers;
  }
  if (options.iceServers !== undefined) {
    return options.iceServers;
  }
  const envIceServers = maybeParseIceServers();
  return envIceServers === null ? undefined : envIceServers;
}

async function fetchJoinMetadata(options: ConnectBrowserTransportOptions): Promise<JoinMetadata> {
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
  const iceServers = normalizeIceServers(payload.ice_servers ?? payload.iceServers);
  const iceServersExpiresAtMs =
    typeof payload.ice_servers_expires_at_ms === 'number'
      ? payload.ice_servers_expires_at_ms
      : typeof (payload as any).iceServersExpiresAtMs === 'number'
        ? (payload as any).iceServersExpiresAtMs
        : undefined;

  return {
    signalingUrl,
    websocketUrl: payload.websocket_url ?? undefined,
    role,
    pollIntervalMs,
    iceServers,
    iceServersExpiresAtMs,
    raw: payload,
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
  // eslint-disable-next-line @typescript-eslint/naming-convention
  ice_servers?: RTCIceServer[];
  // eslint-disable-next-line @typescript-eslint/naming-convention
  ice_servers_expires_at_ms?: number;
  iceServers?: RTCIceServer[];
  iceServersExpiresAtMs?: number;
}
