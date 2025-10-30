import { fetchViewerCredential } from '../lib/api';
import {
  connectBrowserTransport,
  type BrowserTransportConnection,
} from '../../../beach-surfer/src/terminal/connect';
import { TerminalGridStore } from '../../../beach-surfer/src/terminal/gridStore';
import type { TerminalTransport } from '../../../beach-surfer/src/transport/terminalTransport';
import type { SecureTransportSummary } from '../../../beach-surfer/src/transport/webrtc';
import type { HostFrame } from '../../../beach-surfer/src/protocol/types';
import type { SessionCredentialOverride, TerminalViewerState, TerminalViewerStatus } from './terminalViewerTypes';

export type NormalizedOverride = {
  passcode: string | null;
  viewerToken: string | null;
  authorizationToken: string | null;
  skipCredentialFetch: boolean;
};

export type PreparedConnectionParams = {
  sessionId: string;
  privateBeachId: string | null;
  managerUrl: string;
  effectiveAuthToken: string;
  overrides: NormalizedOverride;
  needsCredentialFetch: boolean;
  hasOverrideCredentials: boolean;
};

export type ManagerSnapshot = TerminalViewerState & {
  store: TerminalGridStore;
};

type Subscriber = (snapshot: ManagerSnapshot) => void;

type ListenerBundle = {
  detach: () => void;
  connection: BrowserTransportConnection;
};

type ManagerEntry = {
  key: string;
  params: PreparedConnectionParams;
  store: TerminalGridStore;
  connection: BrowserTransportConnection | null;
  transport: TerminalTransport | null;
  status: TerminalViewerStatus;
  connecting: boolean;
  error: string | null;
  secureSummary: SecureTransportSummary | null;
  latencyMs: number | null;
  lastHeartbeat: number | null;
  subscribers: Set<Subscriber>;
  refCount: number;
  keepAliveTimer: number | null;
  connectPromise: Promise<void> | null;
  reconnectTimer: number | null;
  listenerBundle: ListenerBundle | null;
  disposed: boolean;
};

const entries = new Map<string, ManagerEntry>();
const KEEP_ALIVE_MS = 15_000;
const RECONNECT_DELAY_MS = 1_500;

export function acquireTerminalConnection(
  key: string,
  params: PreparedConnectionParams,
  subscriber: Subscriber,
): () => void {
  const entry = getOrCreateEntry(key, params);
  retainEntry(entry);
  entry.subscribers.add(subscriber);
  subscriber(buildSnapshot(entry));
  ensureEntryConnection(entry);
  return () => {
    entry.subscribers.delete(subscriber);
    releaseEntry(entry);
  };
}

export function normalizeOverride(override?: SessionCredentialOverride): NormalizedOverride {
  return {
    passcode: override?.passcode?.trim() ?? null,
    viewerToken: override?.viewerToken?.trim() ?? null,
    authorizationToken: override?.authorizationToken?.trim() ?? null,
    skipCredentialFetch: override?.skipCredentialFetch ?? false,
  };
}

function getOrCreateEntry(key: string, params: PreparedConnectionParams): ManagerEntry {
  const existing = entries.get(key);
  if (existing) {
    existing.params = params;
    return existing;
  }
  const store = new TerminalGridStore(80);
  store.setFollowTail(true);
  const entry: ManagerEntry = {
    key,
    params,
    store,
    connection: null,
    transport: null,
    status: 'idle',
    connecting: false,
    error: null,
    secureSummary: null,
    latencyMs: null,
    lastHeartbeat: null,
    subscribers: new Set(),
    refCount: 0,
    keepAliveTimer: null,
    connectPromise: null,
    reconnectTimer: null,
    listenerBundle: null,
    disposed: false,
  };
  entries.set(key, entry);
  return entry;
}

function retainEntry(entry: ManagerEntry) {
  if (entry.disposed) {
    entry.disposed = false;
  }
  entry.refCount += 1;
  if (entry.keepAliveTimer != null && typeof window !== 'undefined') {
    window.clearTimeout(entry.keepAliveTimer);
    entry.keepAliveTimer = null;
  }
}

function releaseEntry(entry: ManagerEntry) {
  entry.refCount = Math.max(0, entry.refCount - 1);
  if (entry.refCount === 0) {
    scheduleKeepAlive(entry);
  }
}

function scheduleKeepAlive(entry: ManagerEntry) {
  if (typeof window === 'undefined') {
    disposeEntry(entry, 'keep-alive:ssr');
    return;
  }
  if (entry.keepAliveTimer != null) {
    return;
  }
  entry.keepAliveTimer = window.setTimeout(() => {
    disposeEntry(entry, 'keep-alive:timeout');
  }, KEEP_ALIVE_MS);
}

function disposeEntry(entry: ManagerEntry, reason: string) {
  if (entry.disposed) {
    return;
  }
  entry.disposed = true;
  if (entry.keepAliveTimer != null && typeof window !== 'undefined') {
    window.clearTimeout(entry.keepAliveTimer);
  }
  entry.keepAliveTimer = null;
  if (entry.reconnectTimer != null && typeof window !== 'undefined') {
    window.clearTimeout(entry.reconnectTimer);
  }
  entry.reconnectTimer = null;
  detachListeners(entry);
  closeConnection(entry, reason);
  entries.delete(entry.key);
}

function notifySubscribers(entry: ManagerEntry) {
  const snapshot = buildSnapshot(entry);
  entry.subscribers.forEach((subscriber) => {
    try {
      subscriber(snapshot);
    } catch (err) {
      console.warn('[terminal-manager] subscriber error', err);
    }
  });
}

function buildSnapshot(entry: ManagerEntry): ManagerSnapshot {
  return {
    store: entry.store,
    transport: entry.transport,
    connecting: entry.connecting,
    error: entry.error,
    status: entry.status,
    secureSummary: entry.secureSummary,
    latencyMs: entry.latencyMs,
  };
}

function ensureEntryConnection(entry: ManagerEntry) {
  if (entry.disposed) {
    return;
  }
  if (entry.connection || entry.connectPromise) {
    if (!entry.connectPromise) {
      notifySubscribers(entry);
    }
    return;
  }
  entry.connectPromise = (async () => {
    try {
      entry.connecting = true;
      entry.error = null;
      entry.secureSummary = null;
      entry.latencyMs = null;
      entry.status = entry.status === 'connected' ? 'reconnecting' : 'connecting';
      notifySubscribers(entry);

      const { passcode, viewerToken } = await resolveCredentials(entry);
      const connection = await connectBrowserTransport({
        sessionId: entry.params.sessionId,
        baseUrl: entry.params.managerUrl,
        passcode: passcode ?? undefined,
        viewerToken: viewerToken ?? undefined,
        clientLabel: 'private-beach-dashboard',
        authorizationToken:
          entry.params.effectiveAuthToken.length > 0
            ? entry.params.effectiveAuthToken
            : undefined,
      });
      if (entry.disposed) {
        connection.close();
        return;
      }
      detachListeners(entry);
      entry.connection = connection;
      entry.transport = connection.transport;
      entry.store.reset();
      entry.store.setFollowTail(true);
      entry.connecting = false;
      entry.status = 'connected';
      entry.secureSummary = connection.secure ?? null;
      entry.latencyMs = null;
      entry.lastHeartbeat = null;
      entry.listenerBundle = attachListeners(entry, connection);
      notifySubscribers(entry);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      entry.connecting = false;
      entry.status = 'error';
      entry.error = message;
      entry.transport = null;
      entry.connection = null;
      notifySubscribers(entry);
    } finally {
      entry.connectPromise = null;
    }
  })();
}

async function resolveCredentials(entry: ManagerEntry): Promise<{
  passcode: string | null;
  viewerToken: string | null;
}> {
  if (entry.params.hasOverrideCredentials) {
    const pass = entry.params.overrides.passcode;
    const viewerToken = entry.params.overrides.viewerToken;
    return {
      passcode: pass && pass.length > 0 ? pass : null,
      viewerToken: viewerToken && viewerToken.length > 0 ? viewerToken : null,
    };
  }
  if (entry.params.needsCredentialFetch) {
    const credential = await fetchViewerCredential(
      entry.params.privateBeachId!,
      entry.params.sessionId,
      entry.params.effectiveAuthToken,
      entry.params.managerUrl,
    );
    const credentialType = credential.credential_type?.toLowerCase();
    if (credentialType === 'viewer_token') {
      const viewerToken = credential.credential?.trim() || null;
      const passcode = credential.passcode != null ? String(credential.passcode).trim() : '';
      return {
        passcode: passcode.length > 0 ? passcode : null,
        viewerToken,
      };
    }
    if (credential.credential != null) {
      const pass = String(credential.credential).trim();
      return {
        passcode: pass.length > 0 ? pass : null,
        viewerToken: null,
      };
    }
    throw new Error('viewer passcode unavailable');
  }
  throw new Error('Missing override credentials');
}

function attachListeners(entry: ManagerEntry, connection: BrowserTransportConnection): ListenerBundle {
  const transport = connection.transport;
  const detachFns: Array<() => void> = [];

  const openHandler = () => {
    entry.status = 'connected';
    entry.connecting = false;
    notifySubscribers(entry);
  };
  transport.addEventListener('open', openHandler as EventListener);
  detachFns.push(() => transport.removeEventListener('open', openHandler as EventListener));

  const secureHandler = (event: Event) => {
    const detail = (event as CustomEvent<SecureTransportSummary>).detail;
    entry.secureSummary = detail;
    notifySubscribers(entry);
  };
  transport.addEventListener('secure', secureHandler as EventListener);
  detachFns.push(() => transport.removeEventListener('secure', secureHandler as EventListener));

  const frameHandler = (event: Event) => {
    const detail = (event as CustomEvent<HostFrame>).detail;
    if (detail?.type === 'heartbeat' && typeof detail.timestampMs === 'number') {
      entry.lastHeartbeat = detail.timestampMs;
      const now = Date.now();
      entry.latencyMs = Math.max(0, now - detail.timestampMs);
      notifySubscribers(entry);
    }
  };
  transport.addEventListener('frame', frameHandler as EventListener);
  detachFns.push(() => transport.removeEventListener('frame', frameHandler as EventListener));

  const closeHandler = () => {
    detachListeners(entry);
    entry.connection = null;
    entry.transport = null;
    entry.connecting = true;
    entry.status = 'reconnecting';
    notifySubscribers(entry);
    scheduleReconnect(entry);
  };
  transport.addEventListener('close', closeHandler as EventListener);
  detachFns.push(() => transport.removeEventListener('close', closeHandler as EventListener));

  const errorHandler = (event: Event) => {
    const err = (event as any).error;
    entry.error = err instanceof Error ? err.message : String(err ?? 'transport error');
    entry.status = 'error';
    entry.secureSummary = null;
    entry.transport = null;
    notifySubscribers(entry);
  };
  transport.addEventListener('error', errorHandler as EventListener);
  detachFns.push(() => transport.removeEventListener('error', errorHandler as EventListener));

  const statusHandler = (event: Event) => {
    const detail = (event as CustomEvent<string>).detail;
    if (detail?.toLowerCase().includes('reconnecting')) {
      entry.status = 'reconnecting';
      notifySubscribers(entry);
    }
  };
  transport.addEventListener('status', statusHandler as EventListener);
  detachFns.push(() => transport.removeEventListener('status', statusHandler as EventListener));

  const signalingCloseHandler = () => {
    // Nothing special for now; transport close handler will handle reconnect.
  };
  connection.signaling.addEventListener('close', signalingCloseHandler as EventListener);
  detachFns.push(() =>
    connection.signaling.removeEventListener('close', signalingCloseHandler as EventListener),
  );

  const signalingErrorHandler = (event: Event) => {
    const detail = (event as ErrorEvent).message ?? 'unknown';
    entry.error = detail;
    notifySubscribers(entry);
  };
  connection.signaling.addEventListener('error', signalingErrorHandler as EventListener);
  detachFns.push(() =>
    connection.signaling.removeEventListener('error', signalingErrorHandler as EventListener),
  );

  return {
    connection,
    detach: () => {
      const tasks = detachFns.splice(0);
      for (const task of tasks) {
        try {
          task();
        } catch (err) {
          console.warn('[terminal-manager] detach error', err);
        }
      }
    },
  };
}

function detachListeners(entry: ManagerEntry) {
  if (entry.listenerBundle) {
    entry.listenerBundle.detach();
    entry.listenerBundle = null;
  }
}

function closeConnection(entry: ManagerEntry, reason: string) {
  const connection = entry.connection;
  if (!connection) {
    return;
  }
  try {
    connection.close();
  } catch (err) {
    console.warn('[terminal-manager] error closing connection', reason, err);
  }
  entry.connection = null;
  entry.transport = null;
}

function scheduleReconnect(entry: ManagerEntry) {
  if (entry.disposed || entry.refCount === 0) {
    return;
  }
  if (typeof window === 'undefined') {
    ensureEntryConnection(entry);
    return;
  }
  if (entry.reconnectTimer != null) {
    return;
  }
  entry.reconnectTimer = window.setTimeout(() => {
    entry.reconnectTimer = null;
    ensureEntryConnection(entry);
  }, RECONNECT_DELAY_MS);
}
