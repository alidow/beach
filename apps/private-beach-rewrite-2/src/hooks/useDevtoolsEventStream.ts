'use client';

import { useEffect } from 'react';
import { logConnectionEvent } from '@/features/logging/beachConnectionLogger';
import type { ConnectionLogContext, ConnectionLogLevel } from '@/features/logging/types';

type DevtoolsTimelineEvent = {
  id: string;
  timestamp_ms: number;
  timeline: 'viewer' | 'host' | 'connector';
  step: string;
  level: ConnectionLogLevel;
  session_id?: string | null;
  controller_session_id?: string | null;
  child_session_id?: string | null;
  detail?: Record<string, unknown> | null;
};

type UseDevtoolsEventStreamParams = {
  sessionId?: string | null;
  managerUrl?: string | null;
  authToken?: string | null;
};

const STREAM_EVENT = 'devtools_event';

function normalizeContext(
  event: DevtoolsTimelineEvent,
  fallbackSessionId: string,
): ConnectionLogContext {
  const scope =
    event.timeline === 'host'
      ? 'host'
      : event.timeline === 'connector'
        ? 'connector'
        : 'viewer';
  const controllerId = event.controller_session_id ?? null;
  const childId = event.child_session_id ?? event.session_id ?? fallbackSessionId ?? null;
  const baseSessionId =
    event.timeline === 'connector' ? childId ?? fallbackSessionId : event.session_id ?? fallbackSessionId;
  const connectorLabel =
    scope === 'connector' && controllerId && childId ? `${controllerId} → ${childId}` : undefined;
  return {
    sessionId: baseSessionId ?? fallbackSessionId,
    timelineScope: scope,
    controllerSessionId: controllerId,
    childSessionId: childId,
    timelineGroupLabel: connectorLabel,
    timelineTrackLabel:
      scope === 'host' ? 'Manager↔Host' : scope === 'connector' ? connectorLabel ?? 'Connector' : undefined,
  };
}

export function useDevtoolsEventStream({ sessionId, managerUrl, authToken }: UseDevtoolsEventStreamParams) {
  useEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }
    const trimmedSession = sessionId?.trim();
    const trimmedToken = authToken?.trim();
    const baseUrl = managerUrl?.trim();
    if (!trimmedSession || !trimmedToken || !baseUrl) {
      return;
    }
    const EventSourceCtor = window.EventSource;
    if (typeof EventSourceCtor === 'undefined') {
      return;
    }
    let source: EventSource | null = null;
    let closed = false;
    let retryTimer: number | null = null;

    const closeSource = () => {
      if (source) {
        source.close();
        source = null;
      }
    };

    const scheduleRetry = () => {
      if (closed || retryTimer != null) {
        return;
      }
      closeSource();
      retryTimer = window.setTimeout(() => {
        retryTimer = null;
        openStream();
      }, 2_000);
    };

    const handleEvent = (rawEvent: MessageEvent<string>) => {
      try {
        const payload = JSON.parse(rawEvent.data) as DevtoolsTimelineEvent;
        if (!payload || typeof payload.step !== 'string') {
          return;
        }
        const context = normalizeContext(payload, trimmedSession);
        const detail = payload.detail ?? null;
        logConnectionEvent(payload.step, context, detail ?? undefined, payload.level);
      } catch (error) {
        console.warn('[devtools][timeline] failed to parse devtools event', error);
      }
    };

    const openStream = () => {
      if (closed) {
        return;
      }
      closeSource();
      retryTimer = null;
      let url: URL;
      try {
        url = new URL(
          `/sessions/${encodeURIComponent(trimmedSession)}/devtools/stream`,
          baseUrl,
        );
      } catch (error) {
        console.warn('[devtools][timeline] invalid manager url for devtools stream', error);
        return;
      }
      url.searchParams.set('access_token', trimmedToken);
      const nextSource = new EventSourceCtor(url.toString());
      nextSource.addEventListener(STREAM_EVENT, handleEvent as EventListener);
      nextSource.onmessage = handleEvent;
      nextSource.onopen = () => {
        retryTimer = null;
      };
      nextSource.onerror = () => {
        scheduleRetry();
      };
      source = nextSource;
    };

    openStream();

    return () => {
      closed = true;
      if (retryTimer != null) {
        window.clearTimeout(retryTimer);
        retryTimer = null;
      }
      closeSource();
    };
  }, [authToken, managerUrl, sessionId]);
}
