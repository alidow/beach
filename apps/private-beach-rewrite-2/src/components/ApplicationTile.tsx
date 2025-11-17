'use client';

import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import type { SessionSummary } from '@private-beach/shared-api';
import {
  attachByCode,
  fetchSessionStateSnapshot,
  updateSessionRoleById,
  issueControllerHandshake,
} from '@/lib/api';
import type { ControllerHandshakeResponse } from '@/lib/api';
import type { TileSessionMeta, TileViewportSnapshot } from '@/features/tiles';
import { buildSessionMetadataWithTile, sessionSummaryToTileMeta } from '@/features/tiles/sessionMeta';
import type { SessionCredentialOverride } from '../../../private-beach/src/hooks/terminalViewerTypes';
import { useManagerToken, buildManagerUrl, buildRoadUrl } from '../hooks/useManagerToken';
import { sendControlMessage } from '@/lib/road';
import { useSessionConnection } from '../hooks/useSessionConnection';
import { SessionViewer } from './SessionViewer';
import { logConnectionEvent } from '@/features/logging/beachConnectionLogger';
import {
  hydrateTerminalStoreFromDiff,
  type CellStylePayload,
  type TerminalFramePayload,
  type TerminalStateDiff,
} from '../../../private-beach/src/lib/terminalHydrator';
import type { Update } from '../../../beach-surfer/src/protocol/types';
import { Button } from './ui/button';
import { Input } from './ui/input';
import { Label } from './ui/label';

const DEFAULT_STYLE_ID = 0;
const HANDSHAKE_RENEW_BUFFER_MS = 5_000;
const HANDSHAKE_RENEW_MIN_MS = 5_000;
const HANDSHAKE_RENEW_FALLBACK_MS = 20_000;

function resolveHandshakeRenewMinMs(): number {
  if (typeof globalThis !== 'undefined') {
    const override = (globalThis as Record<string, unknown>).__BEACH_HANDSHAKE_RENEW_MIN_MS__;
    if (typeof override === 'number' && Number.isFinite(override) && override >= 0) {
      return override;
    }
  }
  return HANDSHAKE_RENEW_MIN_MS;
}

function logHydration(event: string, detail: Record<string, unknown>) {
  if (typeof window === 'undefined') {
    return;
  }
  try {
    console.info('[terminal][hydrate]', event, JSON.stringify(detail ?? {}));
  } catch (error) {
    console.info('[terminal][hydrate]', event, {
      fallback: true,
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

function sanitizeStyleId(raw: unknown, fallback = DEFAULT_STYLE_ID): number {
  if (typeof raw === 'number' && Number.isFinite(raw)) {
    const normalized = Math.trunc(raw);
    return normalized >= 0 ? normalized : fallback;
  }
  return fallback;
}

function buildStyleUpdates(styles: CellStylePayload[] | null | undefined, sequence: number): Update[] {
  const updates: Update[] = [];
  const seen = new Set<number>();
  if (Array.isArray(styles)) {
    for (const entry of styles) {
      if (!entry || typeof entry !== 'object') {
        continue;
      }
      const id = sanitizeStyleId(entry.id, DEFAULT_STYLE_ID);
      if (seen.has(id)) {
        continue;
      }
      seen.add(id);
      updates.push({
        type: 'style',
        id,
        seq: sequence,
        fg: typeof entry.fg === 'number' ? entry.fg : 0,
        bg: typeof entry.bg === 'number' ? entry.bg : 0,
        attrs: typeof entry.attrs === 'number' ? entry.attrs : 0,
      });
    }
  }
  if (!seen.has(DEFAULT_STYLE_ID)) {
    updates.push({
      type: 'style',
      id: DEFAULT_STYLE_ID,
      seq: sequence,
      fg: 0,
      bg: 0,
      attrs: 0,
    });
  }
  return updates;
}

function normalizePositiveInteger(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value) && value > 0) {
    return Math.trunc(value);
  }
  return null;
}

function normalizePositiveFloat(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value) && value > 0) {
    return Number(value);
  }
  return null;
}

function inferHostRows(payload: TerminalFramePayload | null | undefined): number | null {
  if (!payload) {
    return null;
  }
  const direct = normalizePositiveInteger(payload.rows);
  if (direct) {
    return direct;
  }
  if (Array.isArray(payload.styled_lines) && payload.styled_lines.length > 0) {
    const count = payload.styled_lines.length;
    return count > 0 ? count : null;
  }
  if (Array.isArray(payload.lines) && payload.lines.length > 0) {
    const count = payload.lines.length;
    return count > 0 ? count : null;
  }
  return null;
}

function inferHostCols(payload: TerminalFramePayload | null | undefined): number | null {
  if (!payload) {
    return null;
  }
  const direct = normalizePositiveInteger(payload.cols);
  if (direct) {
    return direct;
  }
  if (Array.isArray(payload.styled_lines) && payload.styled_lines.length > 0) {
    const maxStyled = payload.styled_lines.reduce((max, row) => {
      if (!Array.isArray(row)) {
        return max;
      }
      return Math.max(max, row.length);
    }, 0);
    if (maxStyled > 0) {
      return maxStyled;
    }
  }
  if (Array.isArray(payload.lines) && payload.lines.length > 0) {
    const maxPlain = payload.lines.reduce((max, line) => {
      if (typeof line !== 'string') {
        return max;
      }
      return Math.max(max, Array.from(line).length);
    }, 0);
    if (maxPlain > 0) {
      return maxPlain;
    }
  }
  return null;
}

type TraceContext = {
  traceId?: string | null;
};

type ApplicationTileProps = {
  tileId: string;
  privateBeachId: string;
  managerUrl?: string;
  roadUrl?: string;
  sessionMeta?: TileSessionMeta | null;
  onSessionMetaChange?: (meta: TileSessionMeta | null) => void;
  disableViewportMeasurements?: boolean;
  onViewportMetricsChange?: (snapshot: TileViewportSnapshot | null) => void;
  traceContext?: TraceContext | null;
};

type SubmitState = 'idle' | 'attaching';

function statusLabel(status: string): string {
  switch (status) {
    case 'connected':
      return 'Connected';
    case 'reconnecting':
      return 'Reconnecting';
    case 'error':
      return 'Error';
    case 'connecting':
    default:
      return 'Connecting';
  }
}

export function ApplicationTile({
  tileId,
  privateBeachId,
  managerUrl = buildManagerUrl(),
  roadUrl,
  sessionMeta,
  onSessionMetaChange,
  disableViewportMeasurements = true,
  onViewportMetricsChange,
  traceContext = null,
}: ApplicationTileProps) {
  const [sessionIdInput, setSessionIdInput] = useState(sessionMeta?.sessionId ?? '');
  const [codeInput, setCodeInput] = useState('');
  const [submitState, setSubmitState] = useState<SubmitState>('idle');
  const [attachError, setAttachError] = useState<string | null>(null);
  const [roleWarning, setRoleWarning] = useState<string | null>(null);
  const [credentialOverride, setCredentialOverride] = useState<SessionCredentialOverride | null>(null);
  const prehydratedSequenceRef = useRef<string | null>(null);
  const cachedStyleUpdatesRef = useRef<Update[] | null>(null);
  const cachedDiffRef = useRef<TerminalStateDiff | null>(null);
  const restoringRef = useRef(false);
  const lastSessionIdRef = useRef<string | null>(sessionMeta?.sessionId ?? null);
  const lastLoggedSessionIdRef = useRef<string | null>(sessionMeta?.sessionId ?? null);
  const lastViewportSnapshotRef = useRef<TileViewportSnapshot | null>(null);
  const hydrationRetryTimerRef = useRef<number | null>(null);
  const hydrationAttemptRef = useRef(0);
  const handshakeRenewTimerRef = useRef<number | null>(null);
  const lastHandshakeContextRef = useRef<{ sessionId: string; passcode: string } | null>(null);
  const handshakeInFlightRef = useRef<{ key: string; promise: Promise<void> } | null>(null);
  const hasDeliveredControlRef = useRef(false);

  const {
    token: managerToken,
    loading: tokenLoading,
    error: tokenError,
    isLoaded,
    isSignedIn,
    refresh,
  } = useManagerToken();

  const resolvedRoadUrl = useMemo(() => {
    if (roadUrl && roadUrl.trim().length > 0) {
      return roadUrl.trim();
    }
    try {
      return buildRoadUrl();
    } catch (error) {
      console.error('[application-tile] missing road url', error);
      return '';
    }
  }, [roadUrl]);

  const clearHandshakeRenewal = useCallback(() => {
    if (handshakeRenewTimerRef.current !== null && typeof window !== 'undefined') {
      window.clearTimeout(handshakeRenewTimerRef.current);
      handshakeRenewTimerRef.current = null;
    }
  }, []);

  const deliverHandshake = useCallback(
    async (sessionId: string, passcode: string, options?: { skipControlMessage?: boolean }) => {
      const key = `${sessionId}::${passcode}`;
      if (handshakeInFlightRef.current?.key === key) {
        return handshakeInFlightRef.current.promise;
      }

      const run = (async () => {
        if (!privateBeachId) {
          throw new Error('Missing private beach identifier for handshake');
        }
        if (!resolvedRoadUrl) {
          throw new Error(
            'Beach Road URL is not configured. Set NEXT_PUBLIC_PRIVATE_BEACH_ROAD_URL or define road_url in Private Beach settings.',
          );
        }

        const ensureToken = async () => {
          if (managerToken && managerToken.trim().length > 0) {
            return managerToken;
          }
          return await refresh();
        };

        const token = await ensureToken();
        if (!token) {
          throw new Error('Unable to fetch manager token for controller handshake');
        }

        const logContext = {
          tileId,
          sessionId,
          privateBeachId,
          managerUrl,
        };
        const isRenewal = Boolean(options?.skipControlMessage);
        const startStep = isRenewal ? 'handshake:renew-start' : 'handshake:start';
        logConnectionEvent(startStep, logContext, {
          hasPasscode: Boolean(passcode && passcode.length > 0),
        });
      let handshake: ControllerHandshakeResponse;
      try {
        handshake = await issueControllerHandshake(
          sessionId,
          passcode,
            privateBeachId,
            token,
            managerUrl,
          );
        logConnectionEvent('handshake:success', logContext, {
          leaseExpiresAtMs: handshake.lease_expires_at_ms ?? null,
        });
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        logConnectionEvent('handshake:error', logContext, { error: message }, 'error');
        throw error;
      }

      const shouldSendControl = !(options?.skipControlMessage ?? false);
      if (shouldSendControl) {
        try {
          const control = await sendControlMessage(
            sessionId,
            'manager_handshake',
            handshake,
            token ?? null,
            resolvedRoadUrl,
          );
          logConnectionEvent('hint:sent', logContext, {
            roadUrl: resolvedRoadUrl,
            controlId: control?.control_id ?? null,
          });
          logConnectionEvent('slow-path:ready', logContext, {
            leaseExpiresAtMs: handshake.lease_expires_at_ms ?? null,
            controlId: control?.control_id ?? null,
          });
          hasDeliveredControlRef.current = true;
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          logConnectionEvent('hint:error', logContext, {
            roadUrl: resolvedRoadUrl,
            error: message,
          }, 'error');
          throw error;
        }
      } else {
        logConnectionEvent('handshake:renew', logContext, {
          leaseExpiresAtMs: handshake.lease_expires_at_ms ?? null,
        });
      }

      lastHandshakeContextRef.current = { sessionId, passcode };

      if (typeof window !== 'undefined') {
        clearHandshakeRenewal();
        const targetExpiry = handshake.lease_expires_at_ms ?? Date.now() + HANDSHAKE_RENEW_FALLBACK_MS;
        const delay = Math.max(resolveHandshakeRenewMinMs(), targetExpiry - Date.now() - HANDSHAKE_RENEW_BUFFER_MS);
        const skipControlOnRenewal = hasDeliveredControlRef.current;
        handshakeRenewTimerRef.current = window.setTimeout(() => {
          void deliverHandshake(sessionId, passcode, { skipControlMessage: skipControlOnRenewal }).catch((error) => {
            console.warn('[application-tile] handshake renewal failed', {
              sessionId,
              error,
            });
              logConnectionEvent(
                'handshake:renew-error',
                logContext,
                { error: error instanceof Error ? error.message : String(error) },
                'warn',
              );
            });
          }, delay);
        }
      })();

      handshakeInFlightRef.current = { key, promise: run };
      try {
        await run;
      } finally {
        if (handshakeInFlightRef.current?.key === key) {
          handshakeInFlightRef.current = null;
        }
      }
    },
    [clearHandshakeRenewal, managerToken, managerUrl, privateBeachId, refresh, resolvedRoadUrl, tileId],
  );

  useEffect(() => {
    if (sessionMeta?.sessionId && sessionMeta.sessionId !== sessionIdInput) {
      setSessionIdInput(sessionMeta.sessionId);
    }
  }, [sessionMeta?.sessionId, sessionIdInput]);

  useEffect(() => {
    const nextSessionId = sessionMeta?.sessionId?.trim() ?? null;
    if (nextSessionId && lastLoggedSessionIdRef.current !== nextSessionId) {
      lastLoggedSessionIdRef.current = nextSessionId;
      logConnectionEvent('session:detected', {
        tileId,
        sessionId: nextSessionId,
        privateBeachId,
        managerUrl,
      }, {
        title: sessionMeta?.title ?? null,
        harnessType: sessionMeta?.harnessType ?? null,
      });
      return;
    }
    if (!nextSessionId && lastLoggedSessionIdRef.current) {
      logConnectionEvent('session:cleared', {
        tileId,
        sessionId: lastLoggedSessionIdRef.current,
        privateBeachId,
        managerUrl,
      });
      lastLoggedSessionIdRef.current = null;
    }
  }, [managerUrl, privateBeachId, sessionMeta?.harnessType, sessionMeta?.sessionId, sessionMeta?.title, tileId]);

  useEffect(() => () => clearHandshakeRenewal(), [clearHandshakeRenewal]);

  useEffect(() => {
    const sessionId = sessionMeta?.sessionId?.trim();
    const passcode = credentialOverride?.passcode?.trim();
    if (!sessionId || !passcode) {
      clearHandshakeRenewal();
      return;
    }
    const ctx = lastHandshakeContextRef.current;
    if (ctx && ctx.sessionId === sessionId && ctx.passcode === passcode) {
      return;
    }
    lastHandshakeContextRef.current = { sessionId, passcode };
    void deliverHandshake(sessionId, passcode).catch((error) => {
      const message = error instanceof Error ? error.message : String(error);
      setAttachError(message);
      console.warn('[application-tile] automatic handshake failed', {
        sessionId,
        error,
      });
      logConnectionEvent('handshake:auto-error', {
        tileId,
        sessionId,
        privateBeachId,
        managerUrl,
      }, { error: message }, 'warn');
    });
  }, [credentialOverride?.passcode, deliverHandshake, clearHandshakeRenewal, managerUrl, privateBeachId, sessionMeta?.sessionId, tileId]);

  useEffect(() => {
    lastViewportSnapshotRef.current = null;
    onViewportMetricsChange?.(null);
    logViewportMetricEvent(tileId, 'reset', {});
    return () => {
      onViewportMetricsChange?.(null);
      logViewportMetricEvent(tileId, 'cleanup', {});
    };
  }, [onViewportMetricsChange, tileId]);

  const resolveIntegerMetric = useCallback(
    (incoming: number | null | undefined, previous: number | null | undefined) => {
      if (incoming === undefined) {
        return previous ?? null;
      }
      if (incoming === null) {
        return null;
      }
      return normalizePositiveInteger(incoming) ?? previous ?? null;
    },
    [],
  );

  const resolveFloatMetric = useCallback(
    (incoming: number | null | undefined, previous: number | null | undefined) => {
      if (incoming === undefined) {
        return previous ?? null;
      }
      if (incoming === null) {
        return null;
      }
      return normalizePositiveFloat(incoming) ?? previous ?? null;
    },
    [],
  );

  const applyViewportPatch = useCallback(
    (patch: Partial<TileViewportSnapshot> | null, source: string) => {
      if (!patch) {
        if (!lastViewportSnapshotRef.current) {
          return;
        }
        lastViewportSnapshotRef.current = null;
        onViewportMetricsChange?.(null);
        logViewportMetricEvent(tileId, 'clear', { source });
        return;
      }
      const previous = lastViewportSnapshotRef.current;
      const next: TileViewportSnapshot = {
        tileId,
        hostRows: resolveIntegerMetric(patch.hostRows, previous?.hostRows),
        hostCols: resolveIntegerMetric(patch.hostCols, previous?.hostCols),
        viewportRows: resolveIntegerMetric(patch.viewportRows, previous?.viewportRows),
        viewportCols: resolveIntegerMetric(patch.viewportCols, previous?.viewportCols),
        pixelsPerRow: resolveFloatMetric(patch.pixelsPerRow, previous?.pixelsPerRow),
        pixelsPerCol: resolveFloatMetric(patch.pixelsPerCol, previous?.pixelsPerCol),
        hostWidthPx: resolveFloatMetric(patch.hostWidthPx, previous?.hostWidthPx),
        hostHeightPx: resolveFloatMetric(patch.hostHeightPx, previous?.hostHeightPx),
        cellWidthPx: resolveFloatMetric(patch.cellWidthPx, previous?.cellWidthPx),
        cellHeightPx: resolveFloatMetric(patch.cellHeightPx, previous?.cellHeightPx),
      };
      if (
        next.hostRows != null &&
        next.hostCols != null &&
        next.hostRows > next.hostCols &&
        next.hostRows >= 80 &&
        next.hostCols <= 80 &&
        next.hostRows >= next.hostCols * 1.2
      ) {
        logViewportMetricEvent(tileId, 'swap-host-dimensions', {
          source,
          hostRows: next.hostRows,
          hostCols: next.hostCols,
        });
        const swapped = next.hostRows;
        next.hostRows = next.hostCols;
        next.hostCols = swapped;
      }
      const same =
        previous &&
        previous.hostRows === next.hostRows &&
        previous.hostCols === next.hostCols &&
        previous.viewportRows === next.viewportRows &&
        previous.viewportCols === next.viewportCols &&
        previous.pixelsPerRow === next.pixelsPerRow &&
        previous.pixelsPerCol === next.pixelsPerCol &&
        previous.hostWidthPx === next.hostWidthPx &&
        previous.hostHeightPx === next.hostHeightPx &&
        previous.cellWidthPx === next.cellWidthPx &&
        previous.cellHeightPx === next.cellHeightPx;
      if (same) {
        return;
      }
      lastViewportSnapshotRef.current = next;
      onViewportMetricsChange?.(next);
      logViewportMetricEvent(tileId, 'update', {
        source,
        hostRows: next.hostRows,
        hostCols: next.hostCols,
        viewportRows: next.viewportRows,
        viewportCols: next.viewportCols,
        pixelsPerRow: next.pixelsPerRow,
        pixelsPerCol: next.pixelsPerCol,
        hostWidthPx: next.hostWidthPx,
        hostHeightPx: next.hostHeightPx,
        cellWidthPx: next.cellWidthPx,
        cellHeightPx: next.cellHeightPx,
      });
    },
    [onViewportMetricsChange, resolveFloatMetric, resolveIntegerMetric, tileId],
  );

  const handleViewportMetrics = useCallback(
    (snapshot: TileViewportSnapshot | null) => {
      if (!snapshot) {
        applyViewportPatch(null, 'terminal');
        return;
      }
      const normalized: Partial<TileViewportSnapshot> = {
        ...snapshot,
        hostRows: snapshot.hostRows ?? undefined,
        hostCols: snapshot.hostCols ?? undefined,
        hostWidthPx: snapshot.hostWidthPx ?? undefined,
        hostHeightPx: snapshot.hostHeightPx ?? undefined,
        cellWidthPx: snapshot.cellWidthPx ?? undefined,
        cellHeightPx: snapshot.cellHeightPx ?? undefined,
      };
      if (
        normalized.hostRows &&
        normalized.hostCols &&
        normalized.pixelsPerRow &&
        normalized.pixelsPerCol &&
        normalized.hostRows > normalized.hostCols * 1.5
      ) {
        logViewportMetricEvent(tileId, 'swap-host-dimensions', {
          source: 'terminal',
          hostRows: normalized.hostRows,
          hostCols: normalized.hostCols,
        });
        const swappedRows = normalized.hostCols;
        const swappedCols = normalized.hostRows;
        normalized.hostRows = swappedRows;
        normalized.hostCols = swappedCols;
        normalized.hostWidthPx = swappedCols * normalized.pixelsPerCol;
        normalized.hostHeightPx = swappedRows * normalized.pixelsPerRow;
      }
      applyViewportPatch(normalized, 'terminal');
    },
    [applyViewportPatch, tileId],
  );

  const viewer = useSessionConnection({
    tileId,
    sessionId: sessionMeta?.sessionId ?? null,
    privateBeachId,
    managerUrl,
    authToken: managerToken,
    credentialOverride: credentialOverride ?? undefined,
    traceContext: traceContext ?? undefined,
  });

  useEffect(() => {
    hasDeliveredControlRef.current = false;
  }, [sessionMeta?.sessionId]);

  useEffect(() => {
    if (viewer.status === 'idle' || viewer.status === 'error') {
      hasDeliveredControlRef.current = false;
    }
  }, [viewer.status]);

  useEffect(() => {
    const currentSessionId = sessionMeta?.sessionId ?? null;
    if (lastSessionIdRef.current !== currentSessionId) {
      lastSessionIdRef.current = currentSessionId;
      prehydratedSequenceRef.current = null;
      cachedStyleUpdatesRef.current = null;
      cachedDiffRef.current = null;
      restoringRef.current = false;
      hydrationAttemptRef.current = 0;
      if (hydrationRetryTimerRef.current != null && typeof window !== 'undefined') {
        window.clearTimeout(hydrationRetryTimerRef.current);
      }
      hydrationRetryTimerRef.current = null;
    }
  }, [sessionMeta?.sessionId]);

  useEffect(() => {
    const store = viewer.store;
    const sessionId = sessionMeta?.sessionId?.trim();
    if (!store || !sessionId || !managerUrl) {
      logHydration('skip:missing-context', {
        hasStore: Boolean(store),
        sessionIdPresent: Boolean(sessionId),
        hasManagerUrl: Boolean(managerUrl),
      });
      return;
    }
    let cancelled = false;
    const scheduleRetry = (reason: string) => {
      if (cancelled || typeof window === 'undefined') {
        return;
      }
      if (hydrationRetryTimerRef.current != null) {
        return;
      }
      const backoffStep = Math.min(hydrationAttemptRef.current, 5);
      const delay = Math.min(1000 * 2 ** backoffStep, 10000);
      hydrationRetryTimerRef.current = window.setTimeout(() => {
        hydrationRetryTimerRef.current = null;
        void fetchAndHydrate();
      }, delay);
      logHydration('retry-scheduled', {
        sessionId,
        reason,
        delayMs: delay,
        attempt: hydrationAttemptRef.current,
      });
    };

    const fetchAndHydrate = async () => {
      hydrationAttemptRef.current += 1;
      let token = managerToken?.trim();
      if (!token) {
        try {
          const refreshed = await refresh();
          token = refreshed?.trim() ?? '';
        } catch (refreshError) {
          logHydration('token-refresh-failed', {
            sessionId,
            error: refreshError instanceof Error ? refreshError.message : String(refreshError),
          });
        }
      }
      if (!token || cancelled) {
        logHydration('skip:no-token', {
          sessionId,
          hadInitialToken: Boolean(managerToken?.trim()),
          cancelled,
        });
        scheduleRetry('no-token');
        return;
      }
      try {
        logHydration('fetch:start', {
          sessionId,
          hasToken: Boolean(token),
          cancelled,
        });
        const diff = await fetchSessionStateSnapshot(sessionId, token, managerUrl);
        if (!diff || cancelled) {
          logHydration(cancelled ? 'skip:cancelled' : 'skip:no-diff', {
            sessionId,
            cancelled,
            attempt: hydrationAttemptRef.current,
          });
          if (!cancelled) {
            scheduleRetry('no-diff');
          }
          return;
        }
        const sequenceKey = `${sessionId}:${diff.sequence ?? 0}`;
        if (prehydratedSequenceRef.current === sequenceKey) {
          logHydration('skip:already-applied', {
            sessionId,
            sequence: diff.sequence ?? 0,
          });
          return;
        }
        const hydrated = hydrateTerminalStoreFromDiff(store, diff, {});
        if (hydrated) {
          hydrationAttemptRef.current = 0;
          prehydratedSequenceRef.current = sequenceKey;
          cachedDiffRef.current = diff;
          cachedStyleUpdatesRef.current = buildStyleUpdates(diff.payload.styles ?? null, diff.sequence ?? 0);
          if (cachedStyleUpdatesRef.current.length > 0) {
            store.applyUpdates(cachedStyleUpdatesRef.current, {
              authoritative: false,
              origin: 'cached-style-refresh',
            });
            logHydration('styles-applied', {
              sessionId,
              sequence: diff.sequence ?? 0,
              count: cachedStyleUpdatesRef.current.length,
            });
          }
          const hostRows = inferHostRows(diff.payload);
          const hostCols = inferHostCols(diff.payload);
          if (hostRows || hostCols) {
            applyViewportPatch(
              {
                tileId,
                hostRows: hostRows ?? undefined,
                hostCols: hostCols ?? undefined,
              },
              'hydrate',
            );
          }
          try {
            const snapshot = store.getSnapshot();
            logHydration('applied-cached-diff', {
              sessionId,
              sequence: diff.sequence ?? 0,
              rows: snapshot.rows.length,
              baseRow: snapshot.baseRow,
              viewportTop: snapshot.viewportTop,
              viewportHeight: snapshot.viewportHeight,
            });
          } catch (error) {
            logHydration('applied-cached-diff', {
              sessionId,
              sequence: diff.sequence ?? 0,
              snapshotError: error instanceof Error ? error.message : String(error),
            });
          }
        } else {
          logHydration('hydrate-returned-false', {
            sessionId,
            sequence: diff.sequence ?? 0,
          });
          scheduleRetry('hydrate-false');
        }
      } catch (error) {
        logHydration('fetch:error', {
          sessionId,
          error: error instanceof Error ? error.message : String(error),
        });
        scheduleRetry('fetch-error');
      }
    };
    fetchAndHydrate();
    return () => {
      cancelled = true;
      if (hydrationRetryTimerRef.current != null && typeof window !== 'undefined') {
        window.clearTimeout(hydrationRetryTimerRef.current);
      }
      hydrationRetryTimerRef.current = null;
    };
  }, [applyViewportPatch, managerToken, managerUrl, refresh, sessionMeta?.sessionId, tileId, viewer.store]);

  useEffect(() => {
    const store = viewer.store;
    if (!store) {
      return undefined;
    }

    const maybeRestoreSnapshot = (reason: string) => {
      if (restoringRef.current) {
        return;
      }
      try {
        const snapshot = store.getSnapshot?.();
        const rows = snapshot?.rows?.length ?? 0;
        const styleCount = snapshot?.styles ? snapshot.styles.size : 0;
        if (rows === 0 && cachedDiffRef.current) {
          restoringRef.current = true;
          const applied = hydrateTerminalStoreFromDiff(store, cachedDiffRef.current, {});
          restoringRef.current = false;
          if (applied) {
            logHydration('diff-restore', {
              sessionId: sessionMeta?.sessionId ?? null,
              reason,
              rowsRestored: cachedDiffRef.current.payload.rows ?? null,
            });
            return;
          }
        }
        const styleUpdates = cachedStyleUpdatesRef.current;
        if (!styleUpdates || styleUpdates.length === 0 || styleCount > 1) {
          return;
        }
        store.applyUpdates(styleUpdates, { authoritative: false, origin: 'cached-style-restore' });
        logHydration('styles-restore', {
          sessionId: sessionMeta?.sessionId ?? null,
          reason,
          count: styleUpdates.length,
          previousStyleCount: styleCount,
        });
      } catch (error) {
        restoringRef.current = false;
        logHydration('restore-error', {
          sessionId: sessionMeta?.sessionId ?? null,
          reason,
          error: error instanceof Error ? error.message : String(error),
        });
      }
    };

    maybeRestoreSnapshot('effect-init');

    if (typeof store.subscribe !== 'function') {
      return undefined;
    }
    const unsubscribe = store.subscribe(() => {
      maybeRestoreSnapshot('store-update');
    });
    return () => {
      try {
        unsubscribe();
      } catch (error) {
        logHydration('restore-unsubscribe-error', {
          sessionId: sessionMeta?.sessionId ?? null,
          error: error instanceof Error ? error.message : String(error),
        });
      }
    };
  }, [sessionMeta?.sessionId, viewer.store]);

  useEffect(() => {
    if (!sessionMeta || !onSessionMetaChange) {
      return;
    }
    const nextStatus = statusLabel(viewer.status);
    if (sessionMeta.status === nextStatus) {
      return;
    }
    onSessionMetaChange({ ...sessionMeta, status: nextStatus });
  }, [sessionMeta, viewer.status, onSessionMetaChange]);

  const handleAttach = useCallback(
    async (event: FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const trimmedSessionId = sessionIdInput.trim();
      const trimmedCode = codeInput.trim();

      if (!privateBeachId) {
        setAttachError('Missing private beach identifier.');
        return;
      }
      if (!trimmedSessionId) {
        setAttachError('Enter a session id before attaching.');
        return;
      }
      if (!trimmedCode) {
        setAttachError('Enter the 6-digit session code.');
        return;
      }

      setSubmitState('attaching');
      setAttachError(null);
      setRoleWarning(null);

      const token =
        managerToken && managerToken.trim().length > 0 ? managerToken : await refresh();
      if (!token) {
        setSubmitState('idle');
        setAttachError('Unable to fetch manager token. Sign in and try again.');
        return;
      }

      try {
        const response = await attachByCode(privateBeachId, trimmedSessionId, trimmedCode, token, managerUrl);
        const session = (response?.session ?? null) as SessionSummary | null;
        if (!session) {
          throw new Error('Attach response missing session payload.');
        }
        const nextMeta = sessionSummaryToTileMeta(session);
        onSessionMetaChange?.(nextMeta);
        setCredentialOverride({ passcode: trimmedCode });
        setCodeInput('');
        setSessionIdInput(session.session_id);
        try {
          await deliverHandshake(session.session_id, trimmedCode);
        } catch (handshakeErr) {
          const message = handshakeErr instanceof Error ? handshakeErr.message : String(handshakeErr);
          setAttachError(message);
          try {
            console.warn('[application-tile] handshake delivery failed', {
              sessionId: session.session_id,
              error: handshakeErr,
            });
          } catch {
            /* noop */
          }
        }

        try {
          const metadataPayload = buildSessionMetadataWithTile(session.metadata, tileId, nextMeta);
          await updateSessionRoleById(
            session.session_id,
            'application',
            token,
            managerUrl,
            metadataPayload,
            session.location_hint ?? null,
          );
        } catch (roleErr) {
          const message = roleErr instanceof Error ? roleErr.message : String(roleErr);
          setRoleWarning(`Attached session, but updating role failed: ${message}`);
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setAttachError(message || 'Failed to attach session.');
      } finally {
        setSubmitState('idle');
      }
    },
    [
      codeInput,
      deliverHandshake,
      managerToken,
      managerUrl,
      onSessionMetaChange,
      privateBeachId,
      refresh,
      sessionIdInput,
      tileId,
    ],
  );

  const disabled = submitState !== 'idle' || tokenLoading;
  const hasSession = Boolean(sessionMeta?.sessionId);
  const sessionIdFieldId = `${tileId}-session-id`;
  const passcodeFieldId = `${tileId}-session-passcode`;

  return (
    <div className="flex h-full min-h-0 flex-col gap-4 text-[13px] text-slate-800 dark:text-slate-200">
      {!hasSession ? (
        <form
          className="space-y-4 rounded-2xl border border-border/70 bg-card/80 px-4 py-5 text-sm text-card-foreground shadow-[0_12px_30px_rgba(2,6,23,0.08)]"
          onSubmit={handleAttach}
        >
          <div className="space-y-2">
            <Label htmlFor={sessionIdFieldId} className="text-[11px]">
              Session ID
            </Label>
            <Input
              id={sessionIdFieldId}
              value={sessionIdInput}
              onChange={(event) => setSessionIdInput(event.target.value)}
              placeholder="sess-1234…"
              autoComplete="off"
              className="h-10 text-[13px] font-medium"
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor={passcodeFieldId} className="text-[11px]">
              Passcode
            </Label>
            <Input
              id={passcodeFieldId}
              value={codeInput}
              onChange={(event) => setCodeInput(event.target.value)}
              placeholder="6-digit code"
              autoComplete="off"
              className="h-10 text-[13px] font-medium"
            />
          </div>
          <Button
            type="submit"
            disabled={disabled}
            className="w-full text-[11px] font-semibold uppercase tracking-[0.18em]"
          >
            {submitState === 'attaching' ? 'Attaching…' : 'Connect'}
          </Button>
          {!isLoaded && <p className="text-[11px] text-slate-400">Loading authentication…</p>}
          {isLoaded && !isSignedIn && (
            <p className="text-[11px] text-slate-400">Sign in with Clerk to request manager credentials.</p>
          )}
          {tokenError && (
            <p className="rounded-xl border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-800 dark:text-red-100">{tokenError}</p>
          )}
          {attachError && (
            <p className="rounded-xl border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-800 dark:text-red-100">{attachError}</p>
          )}
        </form>
      ) : (
        <div className="flex flex-1 min-h-0 flex-col gap-4">
          {roleWarning && (
            <p className="rounded-xl border border-amber-400/40 bg-amber-400/10 px-3 py-2 text-xs text-amber-800 dark:text-amber-100">
              {roleWarning}
            </p>
          )}
          {attachError && (
            <p className="rounded-xl border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-100">{attachError}</p>
          )}
          {viewer.status === 'error' && viewer.error && (
            <p className="rounded-xl border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-100">
              {viewer.error}
            </p>
          )}
          <SessionViewer
            tileId={tileId}
            viewer={viewer}
            sessionId={sessionMeta?.sessionId ?? null}
            disableViewportMeasurements={disableViewportMeasurements}
            onViewportMetrics={handleViewportMetrics}
          />
        </div>
      )}
    </div>
  );
}
function logViewportMetricEvent(
  tileId: string,
  event: string,
  detail: Record<string, unknown>,
) {
  if (typeof window === 'undefined') {
    return;
  }
  try {
    console.info('[tile][viewport]', event, JSON.stringify({ tileId, ...detail }));
  } catch {
    console.info('[tile][viewport]', event, { tileId, ...detail });
  }
}
