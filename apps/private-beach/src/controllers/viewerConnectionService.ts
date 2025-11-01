import {
  acquireTerminalConnection,
  normalizeOverride,
  type ManagerSnapshot,
  type NormalizedOverride,
  type PreparedConnectionParams,
} from '../hooks/sessionTerminalManager';
import type { SessionCredentialOverride, TerminalViewerState, TerminalViewerStatus } from '../hooks/terminalViewerTypes';
import { incrementViewerCounter, resetViewerCounters, type ViewerTileCounters } from './metricsRegistry';

type ConnectionPreparation =
  | {
      ready: false;
      reason: 'no-session-or-url' | 'missing-credentials' | 'missing-override-credentials';
    }
  | {
      ready: true;
      key: string;
      params: PreparedConnectionParams;
      override: NormalizedOverride;
    };

type ConnectionInput = {
  sessionId: string | null | undefined;
  privateBeachId: string | null | undefined;
  managerUrl: string | null | undefined;
  authToken: string | null | undefined;
  override?: SessionCredentialOverride | null | undefined;
};

type TileSubscriber = (snapshot: TerminalViewerState) => void;

type TileEntry = {
  tileId: string;
  subscriber: TileSubscriber;
  key: string | null;
  unsubscribe: (() => void) | null;
  preparation: ConnectionPreparation;
  lastSnapshot: TerminalViewerState;
  lastStatus: TerminalViewerStatus;
  metrics: ViewerTileCounters;
};

const IDLE_STATE: TerminalViewerState & { transportVersion?: number } = {
  store: null,
  transport: null,
  connecting: false,
  error: null,
  status: 'idle',
  secureSummary: null,
  latencyMs: null,
  transportVersion: 0,
};

function toPreparedConnection(input: ConnectionInput): ConnectionPreparation {
  const sessionId = input.sessionId?.trim() ?? '';
  const managerUrl = input.managerUrl?.trim() ?? '';
  const privateBeachId = input.privateBeachId?.trim() ?? null;
  const authToken = input.authToken?.trim() ?? '';
  const override = normalizeOverride(input.override ?? undefined);

  if (sessionId.length === 0 || managerUrl.length === 0) {
    return { ready: false, reason: 'no-session-or-url' };
  }

  const effectiveAuthToken =
    override.authorizationToken && override.authorizationToken.length > 0
      ? override.authorizationToken
      : authToken;

  const hasOverrideCredentials =
    Boolean(override.passcode && override.passcode.length > 0) ||
    Boolean(override.viewerToken && override.viewerToken.length > 0);

  const needsCredentialFetch = !override.skipCredentialFetch && !hasOverrideCredentials;

  if (needsCredentialFetch) {
    const hasPrivateBeach = Boolean(privateBeachId && privateBeachId.length > 0);
    const hasAuthToken = effectiveAuthToken.length > 0;
    if (!hasPrivateBeach || !hasAuthToken) {
      return { ready: false, reason: 'missing-credentials' };
    }
  } else if (!hasOverrideCredentials) {
    return { ready: false, reason: 'missing-override-credentials' };
  }

  const params: PreparedConnectionParams = {
    sessionId,
    privateBeachId,
    managerUrl,
    effectiveAuthToken,
    overrides: override,
    needsCredentialFetch,
    hasOverrideCredentials,
  };

  const key = JSON.stringify({
    sessionId: params.sessionId,
    privateBeachId: params.privateBeachId,
    managerUrl: params.managerUrl,
    authToken: params.effectiveAuthToken,
    passcode: params.overrides.passcode,
    viewerToken: params.overrides.viewerToken,
    skipCredentialFetch: params.overrides.skipCredentialFetch,
  });

  return {
    ready: true,
    key,
    params,
    override,
  };
}

function cloneViewerState(snapshot: TerminalViewerState): TerminalViewerState {
  return {
    store: snapshot.store,
    transport: snapshot.transport,
    connecting: snapshot.connecting,
    error: snapshot.error,
    status: snapshot.status,
    secureSummary: snapshot.secureSummary,
    latencyMs: snapshot.latencyMs,
    transportVersion: (snapshot as any).transportVersion ?? 0,
  };
}

export class ViewerConnectionService {
  private readonly tiles = new Map<string, TileEntry>();

  private record(tileId: string, key: keyof ViewerTileCounters) {
    incrementViewerCounter(tileId, key);
  }

  connectTile(tileId: string, input: ConnectionInput, subscriber: TileSubscriber): () => void {
    const preparation = toPreparedConnection(input);
    const existing = this.tiles.get(tileId);

    if (existing && existing.subscriber !== subscriber) {
      existing.subscriber = subscriber;
    }

    if (!preparation.ready) {
      this.disposeTile(tileId, existing);
      const idle = cloneViewerState(IDLE_STATE);
      subscriber(idle);
      this.tiles.set(tileId, {
        tileId,
        subscriber,
        key: null,
        unsubscribe: null,
        preparation,
        lastSnapshot: idle,
        lastStatus: idle.status,
        metrics:
          existing?.metrics ?? {
            started: 0,
            completed: 0,
            retries: 0,
            failures: 0,
            disposed: 0,
          },
      });
      return () => {
        this.disconnectTile(tileId);
      };
    }

    if (existing && existing.key === preparation.key && existing.unsubscribe) {
      existing.preparation = preparation;
      existing.subscriber = subscriber;
      subscriber(existing.lastSnapshot);
      return () => {
        this.disconnectTile(tileId);
      };
    }

    this.disposeTile(tileId, existing);

    const entry: TileEntry = {
      tileId,
      subscriber,
      key: preparation.key,
      unsubscribe: null,
      preparation,
      lastSnapshot: cloneViewerState(IDLE_STATE),
      lastStatus: 'idle',
      metrics:
        existing?.metrics ?? {
          started: 0,
          completed: 0,
          retries: 0,
          failures: 0,
          disposed: 0,
        },
    };

    entry.unsubscribe = acquireTerminalConnection(preparation.key, preparation.params, (snapshot: ManagerSnapshot) => {
      this.deliverSnapshot(tileId, entry, snapshot);
    });

    this.tiles.set(tileId, entry);
    return () => {
      this.disconnectTile(tileId);
    };
  }

  disconnectTile(tileId: string) {
    const entry = this.tiles.get(tileId);
    this.disposeTile(tileId, entry);
    if (entry) {
      this.tiles.delete(tileId);
    }
  }

  resetMetrics() {
    resetViewerCounters();
  }

  getTileMetrics(tileId: string): ViewerTileCounters {
    const entry = this.tiles.get(tileId);
    if (entry) {
      return { ...entry.metrics };
    }
    return {
      started: 0,
      completed: 0,
      retries: 0,
      failures: 0,
      disposed: 0,
    };
  }

  debugEmit(tileId: string, snapshot: (TerminalViewerState | ManagerSnapshot) & { transportVersion?: number }) {
    if (process.env.NODE_ENV === 'production') {
      return;
    }
    const entry = this.tiles.get(tileId);
    if (!entry) {
      return;
    }
    this.deliverSnapshot(tileId, entry, snapshot);
  }

  private deliverSnapshot(
    tileId: string,
    entry: TileEntry,
    snapshot: (TerminalViewerState | ManagerSnapshot) & { transportVersion?: number },
  ) {
    const prevStatus = entry.lastStatus;
    const prevConnecting = entry.lastSnapshot.connecting;
    const next = cloneViewerState(snapshot as TerminalViewerState);

    if (
      (snapshot.connecting && !prevConnecting) ||
      (snapshot.status === 'reconnecting' && prevStatus !== 'reconnecting')
    ) {
      entry.metrics.started += 1;
      this.record(tileId, 'started');
      if (
        snapshot.status === 'reconnecting' ||
        (snapshot.status === 'connecting' && prevStatus !== 'idle' && prevStatus !== 'connecting')
      ) {
        entry.metrics.retries += 1;
        this.record(tileId, 'retries');
      }
    }

    if (snapshot.status === 'connected' && prevStatus !== 'connected') {
      entry.metrics.completed += 1;
      this.record(tileId, 'completed');
    }

    if (snapshot.status === 'error' && snapshot.error && prevStatus !== 'error') {
      entry.metrics.failures += 1;
      this.record(tileId, 'failures');
    }

    entry.lastSnapshot = next;
    entry.lastStatus = snapshot.status;
    try {
      entry.subscriber(next);
    } catch (error) {
      console.warn('[viewer-service] subscriber error', error);
    }
  }

  private disposeTile(tileId: string, entry: TileEntry | undefined) {
    if (!entry) {
      return;
    }
    if (entry.unsubscribe) {
      try {
        entry.unsubscribe();
      } catch (error) {
        console.warn('[viewer-service] unsubscribe error', { tileId, error });
      }
    }
    entry.metrics.disposed += 1;
    this.record(tileId, 'disposed');
  }
}

export const viewerConnectionService = new ViewerConnectionService();

if (typeof globalThis !== 'undefined' && process.env.NODE_ENV !== 'production') {
  const globalObj = globalThis as Record<string, unknown>;
  if (globalObj.__PRIVATE_BEACH_VIEWER_SERVICE__ !== viewerConnectionService) {
    globalObj.__PRIVATE_BEACH_VIEWER_SERVICE__ = viewerConnectionService;
  }
}
