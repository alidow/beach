import { attachByCode, fetchViewerCredential } from '../lib/api';
import {
  connectBrowserTransport,
  type BrowserTransportConnection,
} from '../../../beach-surfer/src/terminal/connect';
import { TerminalGridStore } from '../../../beach-surfer/src/terminal/gridStore';
import type { TerminalTransport } from '../../../beach-surfer/src/transport/terminalTransport';
import type { SecureTransportSummary } from '../../../beach-surfer/src/transport/webrtc';
import type { HostFrame } from '../../../beach-surfer/src/protocol/types';
import { createConnectionTrace } from '../../../beach-surfer/src/lib/connectionTrace';
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
  reconnectAttempts: number;
  lastCloseReason: string | null;
  lastAttachKey: string | null;
  attachPromise: Promise<void> | null;
};

const entries = new Map<string, ManagerEntry>();
const KEEP_ALIVE_MS = 15_000;
const RECONNECT_DELAY_MS = 1_500;
const MAX_RECONNECT_DELAY_MS = 15_000;

type SnapshotSummary = {
  followTail: boolean | null;
  baseRow: number | null;
  viewportHeight: number | null;
  rowCount: number | null;
};

type FollowTailDecision = {
  enable: boolean;
  reason: string;
};

function debugLog(message: string, detail?: Record<string, unknown>) {
  if (typeof window === 'undefined') {
    return;
  }
  if (process.env.NODE_ENV === 'production') {
    return;
  }
  const payload = detail ? JSON.stringify(detail) : '';
  // eslint-disable-next-line no-console
  console.info(`[terminal-manager][rewrite] ${message}${payload ? ` ${payload}` : ''}`);
}

async function ensureSessionAttached(entry: ManagerEntry, passcode: string | null): Promise<void> {
  const trimmedPasscode = passcode?.trim();
  if (!trimmedPasscode || trimmedPasscode.length === 0) {
    return;
  }
  const privateBeachId = entry.params.privateBeachId?.trim();
  if (!privateBeachId || privateBeachId.length === 0) {
    return;
  }
  const authToken = entry.params.effectiveAuthToken?.trim();
  if (!authToken || authToken.length === 0) {
    return;
  }
  const attachKey = `${privateBeachId}:${entry.params.sessionId}:${trimmedPasscode}`;
  if (entry.lastAttachKey === attachKey) {
    return;
  }
  if (entry.attachPromise) {
    try {
      await entry.attachPromise;
    } catch {
      // ignore prior failure and retry below
    }
    if (entry.lastAttachKey === attachKey) {
      return;
    }
  }
  entry.attachPromise = (async () => {
    try {
      debugLog('session.attach_by_code.start', {
        sessionId: entry.params.sessionId,
        privateBeachId,
      });
      await attachByCode(privateBeachId, entry.params.sessionId, trimmedPasscode, authToken, entry.params.managerUrl);
      entry.lastAttachKey = attachKey;
      debugLog('session.attach_by_code.success', {
        sessionId: entry.params.sessionId,
        privateBeachId,
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      // Treat duplicate attaches as success; the mapping already exists.
      if (message.includes('409')) {
        entry.lastAttachKey = attachKey;
        debugLog('session.attach_by_code.duplicate', {
          sessionId: entry.params.sessionId,
          privateBeachId,
        });
        return;
      }
      debugLog('session.attach_by_code.error', {
        sessionId: entry.params.sessionId,
        privateBeachId,
        message,
      });
    } finally {
      entry.attachPromise = null;
    }
  })();
  try {
    await entry.attachPromise;
  } catch {
    // Swallow attach failures to avoid blocking viewer connection attempts.
  }
}

function summarizeStoreSnapshot(store: TerminalGridStore): SnapshotSummary | null {
  try {
    const snapshot = store.getSnapshot();
    return {
      followTail: snapshot.followTail,
      baseRow: snapshot.baseRow,
      viewportHeight: snapshot.viewportHeight,
      rowCount: snapshot.rows.length,
    };
  } catch (error) {
    debugLog('followTail.snapshot_error', {
      message: error instanceof Error ? error.message : String(error),
    });
    return null;
  }
}

function decideFollowTailRestore(summary: SnapshotSummary | null): FollowTailDecision {
  if (!summary) {
    return { enable: true, reason: 'auto-tail:no-snapshot' };
  }
  if (summary.followTail) {
    return { enable: true, reason: 'preserve-follow-tail' };
  }
  const rowCount = summary.rowCount ?? 0;
  if (rowCount <= 0) {
    return { enable: true, reason: 'auto-tail:empty-grid' };
  }
  const viewportHeight = summary.viewportHeight ?? 0;
  if (viewportHeight > 0 && rowCount <= viewportHeight) {
    return { enable: true, reason: 'auto-tail:grid-fits-viewport' };
  }
  return { enable: false, reason: 'preserve-manual-scroll' };
}

function applyFollowTailState(entry: ManagerEntry, enable: boolean, reason: string, summary?: SnapshotSummary | null) {
  const snapshot = summary ?? summarizeStoreSnapshot(entry.store);
  const changed = entry.store.setFollowTail(enable);
  if (!enable) {
    return;
  }
  debugLog('followTail.set', {
    key: entry.key,
    sessionId: entry.params.sessionId,
    reason,
    applied: changed,
    previousFollowTail: snapshot?.followTail ?? null,
    baseRow: snapshot?.baseRow ?? null,
    viewportHeight: snapshot?.viewportHeight ?? null,
    rowCount: snapshot?.rowCount ?? null,
  });
}

function logFollowTailSkip(entry: ManagerEntry, reason: string, summary: SnapshotSummary | null) {
  debugLog('followTail.skip_enable', {
    key: entry.key,
    sessionId: entry.params.sessionId,
    reason,
    previousFollowTail: summary?.followTail ?? null,
    baseRow: summary?.baseRow ?? null,
    viewportHeight: summary?.viewportHeight ?? null,
    rowCount: summary?.rowCount ?? null,
  });
}

export function acquireTerminalConnection(
  key: string,
  params: PreparedConnectionParams,
  subscriber: Subscriber,
): () => void {
  const entry = getOrCreateEntry(key, params);
  debugLog('acquireTerminalConnection', {
    key,
    sessionId: params.sessionId,
    privateBeachId: params.privateBeachId,
    managerUrl: params.managerUrl,
    hasAuthToken: params.effectiveAuthToken.length > 0,
    hasOverrideCredentials: params.hasOverrideCredentials,
    needsCredentialFetch: params.needsCredentialFetch,
    existingSubscribers: entry.subscribers.size,
  });
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
    reconnectAttempts: 0,
    lastCloseReason: null,
    lastAttachKey: null,
    attachPromise: null,
  };
  applyFollowTailState(entry, true, 'entry-init');
  entries.set(key, entry);
  debugLog('managerEntry.created', {
    key,
    sessionId: params.sessionId,
    privateBeachId: params.privateBeachId,
    managerUrl: params.managerUrl,
  });
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
  debugLog('managerEntry.disposed', {
    key: entry.key,
    sessionId: entry.params.sessionId,
    reason,
  });
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
  debugLog('ensureEntryConnection.start', {
    key: entry.key,
    sessionId: entry.params.sessionId,
    status: entry.status,
  });
  entry.connectPromise = (async () => {
    const connectionTrace = createConnectionTrace({
      sessionId: entry.params.sessionId,
      baseUrl: entry.params.managerUrl,
    });
    try {
      entry.connecting = true;
      entry.error = null;
      entry.secureSummary = null;
      entry.latencyMs = null;
      entry.status = entry.status === 'connected' ? 'reconnecting' : 'connecting';
      notifySubscribers(entry);

      const { passcode, viewerToken } = await resolveCredentials(entry);
      debugLog('ensureEntryConnection.credentials', {
        key: entry.key,
        sessionId: entry.params.sessionId,
        hasPasscode: Boolean(passcode),
        hasViewerToken: Boolean(viewerToken),
        fetched: !entry.params.hasOverrideCredentials,
      });
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
        trace: connectionTrace ?? undefined,
      });
      if (entry.disposed) {
        connectionTrace?.finish('cancelled', { reason: 'entry-disposed' });
        connection.close();
        return;
      }
      detachListeners(entry);
      entry.connection = connection;
      entry.transport = connection.transport;
      const snapshotBeforeReset = summarizeStoreSnapshot(entry.store);
      entry.store.reset();
      const followTailDecision = decideFollowTailRestore(snapshotBeforeReset);
      if (followTailDecision.enable) {
        applyFollowTailState(entry, true, followTailDecision.reason, snapshotBeforeReset);
      } else {
        logFollowTailSkip(entry, followTailDecision.reason, snapshotBeforeReset);
      }
      entry.connecting = false;
      entry.status = 'connected';
      entry.secureSummary = connection.secure ?? null;
      entry.latencyMs = null;
      entry.lastHeartbeat = null;
      entry.listenerBundle = attachListeners(entry, connection);
      entry.reconnectAttempts = 0;
      entry.lastCloseReason = null;
      notifySubscribers(entry);
      connectionTrace?.finish('success', {
        remotePeerId: connection.remotePeerId ?? null,
        secureMode: connection.secure?.mode ?? 'plaintext',
      });
      debugLog('ensureEntryConnection.connected', {
        key: entry.key,
        sessionId: entry.params.sessionId,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      entry.connecting = false;
      entry.status = 'error';
      entry.error = message;
      entry.transport = null;
      entry.connection = null;
      notifySubscribers(entry);
      entry.reconnectAttempts = Math.min(entry.reconnectAttempts + 1, 8);
      entry.lastCloseReason = message;
      scheduleReconnect(entry, { immediate: false });
      connectionTrace?.finish('error', {
        error: message,
      });
      debugLog('ensureEntryConnection.error', {
        key: entry.key,
        sessionId: entry.params.sessionId,
        message,
      });
    } finally {
      entry.connectPromise = null;
      debugLog('ensureEntryConnection.finalize', {
        key: entry.key,
        sessionId: entry.params.sessionId,
        disposed: entry.disposed,
      });
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
    if (pass && pass.length > 0) {
      await ensureSessionAttached(entry, pass);
    }
    debugLog('resolveCredentials.override', {
      key: entry.key,
      sessionId: entry.params.sessionId,
      hasPasscode: Boolean(pass),
      hasViewerToken: Boolean(viewerToken),
    });
    return {
      passcode: pass && pass.length > 0 ? pass : null,
      viewerToken: viewerToken && viewerToken.length > 0 ? viewerToken : null,
    };
  }
  if (entry.params.needsCredentialFetch) {
    debugLog('resolveCredentials.fetch_start', {
      key: entry.key,
      sessionId: entry.params.sessionId,
      privateBeachId: entry.params.privateBeachId,
    });
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
      debugLog('resolveCredentials.fetch_success', {
        key: entry.key,
        sessionId: entry.params.sessionId,
        credentialType,
        hasPasscode: passcode.length > 0,
        hasViewerToken: Boolean(viewerToken),
      });
      if (passcode.length > 0) {
        await ensureSessionAttached(entry, passcode);
      }
      return {
        passcode: passcode.length > 0 ? passcode : null,
        viewerToken,
      };
    }
    if (credential.credential != null) {
      const pass = String(credential.credential).trim();
      debugLog('resolveCredentials.fetch_success', {
        key: entry.key,
        sessionId: entry.params.sessionId,
        credentialType: 'passcode',
        hasPasscode: pass.length > 0,
        hasViewerToken: false,
      });
      return {
        passcode: pass.length > 0 ? pass : null,
        viewerToken: null,
      };
    }
    debugLog('resolveCredentials.fetch_failure', {
      key: entry.key,
      sessionId: entry.params.sessionId,
      reason: 'viewer passcode unavailable',
    });
    throw new Error('viewer passcode unavailable');
  }
  debugLog('resolveCredentials.fetch_failure', {
    key: entry.key,
    sessionId: entry.params.sessionId,
    reason: 'missing override credentials',
  });
  throw new Error('Missing override credentials');
}

function attachListeners(entry: ManagerEntry, connection: BrowserTransportConnection): ListenerBundle {
  const transport = connection.transport;
  const detachFns: Array<() => void> = [];

  const openHandler = () => {
    entry.status = 'connected';
    entry.connecting = false;
    notifySubscribers(entry);
    debugLog('transport.open', {
      key: entry.key,
      sessionId: entry.params.sessionId,
    });
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

  const closeHandler = (event: Event) => {
    const eventReason =
      typeof (event as any)?.reason === 'string'
        ? String((event as any).reason)
        : typeof (event as any)?.detail?.reason === 'string'
          ? String((event as any).detail.reason)
          : null;
    detachListeners(entry);
    entry.connection = null;
    entry.transport = null;
    entry.connecting = true;
    entry.status = 'reconnecting';
    entry.reconnectAttempts = Math.min(entry.reconnectAttempts + 1, 8);
    entry.lastCloseReason = eventReason ?? 'transport-close';
    notifySubscribers(entry);
    scheduleReconnect(entry, { reason: entry.lastCloseReason ?? undefined });
    debugLog('transport.close', {
      key: entry.key,
      sessionId: entry.params.sessionId,
      reason: entry.lastCloseReason,
    });
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
    debugLog('transport.error', {
      key: entry.key,
      sessionId: entry.params.sessionId,
      message: entry.error,
    });
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

function scheduleReconnect(entry: ManagerEntry, options?: { reason?: string; immediate?: boolean }) {
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
  const attempt = Math.max(0, entry.reconnectAttempts - 1);
  const computedDelay = Math.min(
    RECONNECT_DELAY_MS * (attempt > 0 ? 2 ** attempt : 1),
    MAX_RECONNECT_DELAY_MS,
  );
  const delay = options?.immediate ? 0 : computedDelay;
  if (typeof window !== 'undefined') {
    try {
      console.info('[terminal-manager] schedule-reconnect', {
        sessionId: entry.params.sessionId,
        attempt: entry.reconnectAttempts,
        delay,
        reason: options?.reason ?? entry.lastCloseReason ?? null,
      });
    } catch {
      // ignore logging issues
    }
  }
  entry.reconnectTimer = window.setTimeout(() => {
    entry.reconnectTimer = null;
    ensureEntryConnection(entry);
  }, delay);
}
