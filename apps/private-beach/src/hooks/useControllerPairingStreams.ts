import { useEffect } from 'react';
import {
  normalizeControllerPairing,
  sortControllerPairings,
  type ControllerPairing,
} from '../lib/api';
import { debugLog } from '../lib/debug';

const SSE_RETRY_BASE_DELAY_MS = 1_000;
const SSE_RETRY_MAX_DELAY_MS = 30_000;

type Params = {
  managerUrl: string | null;
  managerToken: string | null;
  controllerSessionIds: string[];
  setAssignments: React.Dispatch<React.SetStateAction<ControllerPairing[]>>;
};

type ControllerPairingStreamEvent = {
  controller_session_id?: string;
  child_session_id?: string;
  action?: string;
  pairing?: unknown;
};

export type ControllerPairingEventAction = 'added' | 'updated' | 'removed';

export function applyControllerPairingEvent(
  prev: ControllerPairing[],
  {
    controllerId,
    childId,
    action,
    pairing,
  }: {
    controllerId: string;
    childId: string;
    action: ControllerPairingEventAction;
    pairing?: unknown;
  },
): ControllerPairing[] {
  const map = new Map<string, ControllerPairing>();
  prev.forEach((existing) => {
    map.set(`${existing.controller_session_id}|${existing.child_session_id}`, existing);
  });
  if (action === 'removed') {
    map.delete(`${controllerId}|${childId}`);
    return sortControllerPairings(Array.from(map.values()));
  }
  if (!pairing) {
    return sortControllerPairings(Array.from(map.values()));
  }
  try {
    const normalized = normalizeControllerPairing(pairing);
    map.set(`${normalized.controller_session_id}|${normalized.child_session_id}`, normalized);
  } catch (err) {
    debugLog(
      'pairing-sse',
      'failed to normalize pairing',
      {
        controllerId,
        childId,
        error: err instanceof Error ? err.message : String(err),
      },
      'warn',
    );
  }
  return sortControllerPairings(Array.from(map.values()));
}

export function useControllerPairingStreams({
  managerUrl,
  managerToken,
  controllerSessionIds,
  setAssignments,
}: Params) {
  useEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }
    const trimmedToken = managerToken?.trim() ?? '';
    if (!trimmedToken || trimmedToken.length === 0) {
      return;
    }
    if (!managerUrl || managerUrl.trim().length === 0) {
      return;
    }
    if (controllerSessionIds.length === 0) {
      return;
    }
    const EventSourceCtor = window.EventSource;
    if (typeof EventSourceCtor === 'undefined') {
      debugLog(
        'pairing-sse',
        'EventSource unavailable; skipping stream subscription',
        {
          managerUrl,
        },
        'warn',
      );
      return;
    }
    let cancelled = false;
    const activeSources = new Map<string, EventSource>();
    const retryTimers = new Map<string, number>();
    const retryCounts = new Map<string, number>();

    const closeStream = (controllerId: string) => {
      const source = activeSources.get(controllerId);
      if (source) {
        source.close();
        activeSources.delete(controllerId);
      }
      const timerId = retryTimers.get(controllerId);
      if (typeof timerId === 'number') {
        window.clearTimeout(timerId);
        retryTimers.delete(controllerId);
      }
    };

    const scheduleRetry = (controllerId: string) => {
      if (cancelled) {
        return;
      }
      const attempts = (retryCounts.get(controllerId) ?? 0) + 1;
      retryCounts.set(controllerId, attempts);
      const delay = Math.min(
        SSE_RETRY_MAX_DELAY_MS,
        SSE_RETRY_BASE_DELAY_MS * Math.pow(2, attempts - 1),
      );
      debugLog(
        'pairing-sse',
        'schedule reconnect',
        {
          controllerId,
          delay,
          attempts,
        },
        attempts > 3 ? 'warn' : 'info',
      );
      const timerId = window.setTimeout(() => {
        retryTimers.delete(controllerId);
        startStream(controllerId);
      }, delay);
      retryTimers.set(controllerId, timerId);
    };

    const startStream = (controllerId: string) => {
      if (cancelled) {
        return;
      }
      closeStream(controllerId);
      let streamUrl: URL;
      try {
        streamUrl = new URL(`/sessions/${encodeURIComponent(controllerId)}/controllers/stream`, managerUrl);
      } catch (err: any) {
        debugLog(
          'pairing-sse',
          'invalid manager url',
          {
            controllerId,
            managerUrl,
            error: err?.message ?? String(err),
          },
          'warn',
        );
        return;
      }
      streamUrl.searchParams.set('access_token', trimmedToken);
      const source = new EventSourceCtor(streamUrl.toString());
      activeSources.set(controllerId, source);
      debugLog('pairing-sse', 'stream opened', {
        controllerId,
        managerUrl,
      });

      const handleEvent = (event: MessageEvent<string>) => {
        if (cancelled) return;
        try {
          const parsed = JSON.parse(event.data) as ControllerPairingStreamEvent;
          const action = typeof parsed.action === 'string' ? parsed.action : '';
          const controller =
            typeof parsed.controller_session_id === 'string'
              ? parsed.controller_session_id
              : controllerId;
          const child =
            typeof parsed.child_session_id === 'string' ? parsed.child_session_id : null;
          if (!child) {
            debugLog(
              'pairing-sse',
              'event missing child id',
              {
                controllerId: controller,
                raw: parsed,
              },
              'warn',
            );
            return;
          }
          if (action !== 'added' && action !== 'updated' && action !== 'removed') {
            debugLog(
              'pairing-sse',
              'unknown action',
              {
                controllerId: controller,
                childSessionId: child,
                action,
              },
              'warn',
            );
            return;
          }
          debugLog('pairing-sse', 'event received', {
            controllerId: controller,
            childSessionId: child,
            action,
          });
          setAssignments((prev) =>
            applyControllerPairingEvent(prev, {
              controllerId: controller,
              childId: child,
              action: action as ControllerPairingEventAction,
              pairing: parsed.pairing,
            }),
          );
        } catch (err: any) {
          debugLog(
            'pairing-sse',
            'failed to parse event',
            {
              controllerId,
              error: err?.message ?? String(err),
              data: event.data,
            },
            'warn',
          );
        }
      };

      source.addEventListener('controller_pairing', handleEvent as EventListener);

      source.onopen = () => {
        retryCounts.delete(controllerId);
        const timerId = retryTimers.get(controllerId);
        if (typeof timerId === 'number') {
          window.clearTimeout(timerId);
          retryTimers.delete(controllerId);
        }
        debugLog('pairing-sse', 'stream ready', {
          controllerId,
        });
      };

      source.onerror = () => {
        debugLog(
          'pairing-sse',
          'stream error',
          {
            controllerId,
            readyState: source.readyState,
          },
          'warn',
        );
        closeStream(controllerId);
        scheduleRetry(controllerId);
      };
    };

    controllerSessionIds.forEach((controllerId) => {
      startStream(controllerId);
    });

    return () => {
      cancelled = true;
      controllerSessionIds.forEach((controllerId) => {
        closeStream(controllerId);
      });
      retryTimers.forEach((timerId) => {
        window.clearTimeout(timerId);
      });
      activeSources.clear();
      retryTimers.clear();
      retryCounts.clear();
    };
  }, [controllerSessionIds, managerToken, managerUrl, setAssignments]);
}
