'use client';

import type { ConnectionLogContext, ConnectionLogDetail, ConnectionLogLevel } from './types';
import { recordConnectionTimelineEvent } from '@/features/devtools/connectionTimelineStore';

const LOG_PREFIX = '[beach-connection]';
const CONNECTION_TRACE_ENV = 'NEXT_PUBLIC_BEACH_CONNECTION_TRACE';

type TraceHost = typeof globalThis & {
  __BEACH_TRACE?: boolean;
  BEACH_TRACE?: boolean;
  __BEACH_CONNECTION_TRACE?: boolean;
  BEACH_CONNECTION_TRACE?: boolean;
};

function safeSerialize(payload: Record<string, unknown>) {
  try {
    return JSON.stringify(payload);
  } catch {
    return payload;
  }
}

function sanitizeDetail(detail?: ConnectionLogDetail): Record<string, unknown> | undefined {
  if (!detail) {
    return undefined;
  }
  const normalized: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(detail)) {
    if (value === undefined) {
      continue;
    }
    if (value instanceof Error) {
      normalized[key] = { message: value.message, name: value.name };
      continue;
    }
    normalized[key] = value as unknown;
  }
  return normalized;
}

function readFlag(host: TraceHost | null, key: keyof TraceHost): boolean {
  if (!host) {
    return false;
  }
  return Boolean(host[key]);
}

function readWindowFlag(key: keyof TraceHost): boolean {
  if (typeof window === 'undefined') {
    return false;
  }
  return Boolean((window as TraceHost)[key]);
}

export function isBeachTraceEnabled(): boolean {
  if (typeof globalThis === 'undefined') {
    return false;
  }
  const host = globalThis as TraceHost;
  if (readFlag(host, '__BEACH_TRACE') || readFlag(host, 'BEACH_TRACE')) {
    return true;
  }
  return readWindowFlag('__BEACH_TRACE') || readWindowFlag('BEACH_TRACE');
}

function isConnectionTraceEnabled(): boolean {
  if (isBeachTraceEnabled()) {
    return true;
  }
  const host = (typeof globalThis !== 'undefined' ? (globalThis as TraceHost) : null);
  if (
    readFlag(host, '__BEACH_CONNECTION_TRACE') ||
    readFlag(host, 'BEACH_CONNECTION_TRACE') ||
    readWindowFlag('__BEACH_CONNECTION_TRACE') ||
    readWindowFlag('BEACH_CONNECTION_TRACE')
  ) {
    return true;
  }
  if (typeof process !== 'undefined' && process.env?.[CONNECTION_TRACE_ENV] === '1') {
    return true;
  }
  return false;
}

export function logConnectionEvent(
  step: string,
  context: ConnectionLogContext = {},
  detail?: ConnectionLogDetail,
  level: ConnectionLogLevel = 'info',
) {
  if (typeof console === 'undefined') {
    return;
  }
  const payload: Record<string, unknown> = {
    ...context,
    timestamp: new Date().toISOString(),
  };
  const normalizedDetail = sanitizeDetail(detail);
  if (normalizedDetail) {
    payload.detail = normalizedDetail;
  }
  const logger = level === 'error' ? console.error : level === 'warn' ? console.warn : console.info;
  logger(LOG_PREFIX, step, safeSerialize(payload));
  recordConnectionTimelineEvent(step, context, normalizedDetail ?? null, level);
  if (isConnectionTraceEnabled()) {
    console.debug(`${LOG_PREFIX}[trace] ${step}`, payload);
  }
}
