'use client';

import { useEffect, useMemo, useState } from 'react';
import type { SessionCredentialOverride, TerminalViewerState, TerminalViewerStatus } from './terminalViewerTypes';
import {
  acquireTerminalConnection,
  normalizeOverride,
  type NormalizedOverride,
  type PreparedConnectionParams,
} from './sessionTerminalManager';

export type { SessionCredentialOverride, TerminalViewerState, TerminalViewerStatus } from './terminalViewerTypes';

type PreparedConnection =
  | {
      ready: false;
      reason: 'no-session-or-url' | 'missing-credentials' | 'missing-override-credentials';
    }
  | {
      ready: true;
      key: string;
      params: PreparedConnectionParams;
    };

const IDLE_STATE: TerminalViewerState = {
  store: null,
  transport: null,
  connecting: false,
  error: null,
  status: 'idle',
  secureSummary: null,
  latencyMs: null,
};

export function useSessionTerminal(
  sessionId: string | null | undefined,
  privateBeachId: string | null | undefined,
  managerUrl: string,
  token: string | null,
  override?: SessionCredentialOverride,
): TerminalViewerState {
  const trimmedSessionId = sessionId?.trim() ?? '';
  const trimmedPrivateBeachId = privateBeachId?.trim() ?? null;
  const trimmedManagerUrl = managerUrl.trim();
  const trimmedToken = token?.trim() ?? '';
  const normalizedOverride = useMemo(() => normalizeOverride(override), [override?.authorizationToken, override?.passcode, override?.skipCredentialFetch, override?.viewerToken]);

  const prepared = useMemo<PreparedConnection>(() => {
    if (trimmedSessionId.length === 0 || trimmedManagerUrl.length === 0) {
      return { ready: false, reason: 'no-session-or-url' };
    }

    const params = prepareConnectionParams(
      trimmedSessionId,
      trimmedPrivateBeachId,
      trimmedManagerUrl,
      trimmedToken,
      normalizedOverride,
    );

    if (!params) {
      return { ready: false, reason: 'missing-credentials' };
    }

    const { connectionParams, needsOverrideCredentials } = params;

    if (needsOverrideCredentials || !connectionParams) {
      return { ready: false, reason: 'missing-override-credentials' };
    }

    const cacheKey = JSON.stringify({
      sessionId: connectionParams.sessionId,
      privateBeachId: connectionParams.privateBeachId,
      managerUrl: connectionParams.managerUrl,
      authToken: connectionParams.effectiveAuthToken,
      passcode: connectionParams.overrides.passcode,
      viewerToken: connectionParams.overrides.viewerToken,
      skipCredentialFetch: connectionParams.overrides.skipCredentialFetch,
    });

    return {
      ready: true,
      key: cacheKey,
      params: connectionParams,
    };
  }, [
    trimmedSessionId,
    trimmedPrivateBeachId,
    trimmedManagerUrl,
    trimmedToken,
    normalizedOverride.passcode,
    normalizedOverride.viewerToken,
    normalizedOverride.authorizationToken,
    normalizedOverride.skipCredentialFetch,
  ]);

  const [viewerState, setViewerState] = useState<TerminalViewerState>(IDLE_STATE);

  useEffect(() => {
    if (!prepared.ready) {
      setViewerState(IDLE_STATE);
      return;
    }

    let active = true;
    const unsubscribe = acquireTerminalConnection(prepared.key, prepared.params, (snapshot) => {
      if (!active) {
        return;
      }
      setViewerState({
        store: snapshot.store,
        transport: snapshot.transport,
        connecting: snapshot.connecting,
        error: snapshot.error,
        status: snapshot.status,
        secureSummary: snapshot.secureSummary,
        latencyMs: snapshot.latencyMs,
      });
    });

    return () => {
      active = false;
      unsubscribe();
    };
  }, [prepared]);

  return viewerState;
}

function prepareConnectionParams(
  sessionId: string,
  privateBeachId: string | null,
  managerUrl: string,
  token: string,
  overrides: NormalizedOverride,
):
  | {
      connectionParams: PreparedConnectionParams;
      needsOverrideCredentials: false;
    }
  | {
      connectionParams: null;
      needsOverrideCredentials: true;
    }
  | null {
  const effectiveAuthToken =
    overrides.authorizationToken && overrides.authorizationToken.length > 0
      ? overrides.authorizationToken
      : token;

  const hasOverrideCredentials =
    Boolean(overrides.passcode && overrides.passcode.length > 0) ||
    Boolean(overrides.viewerToken && overrides.viewerToken.length > 0);
  const needsCredentialFetch = !overrides.skipCredentialFetch && !hasOverrideCredentials;

  if (needsCredentialFetch) {
    const hasPrivateBeach = Boolean(privateBeachId && privateBeachId.length > 0);
    const hasAuthToken = effectiveAuthToken.length > 0;
    if (!hasPrivateBeach || !hasAuthToken) {
      return null;
    }
  } else if (!hasOverrideCredentials) {
    return { connectionParams: null, needsOverrideCredentials: true };
  }

  const connectionParams: PreparedConnectionParams = {
    sessionId,
    privateBeachId,
    managerUrl,
    effectiveAuthToken,
    overrides,
    needsCredentialFetch,
    hasOverrideCredentials,
  };

  return {
    connectionParams,
    needsOverrideCredentials: false,
  };
}
