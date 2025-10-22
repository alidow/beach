const STORAGE_KEY = 'privateBeach:debug';
const GLOBAL_FLAG = '__PRIVATE_BEACH_DEBUG__';
const QUERY_FLAG = 'pbDebug';

type DebugLevel = 'info' | 'warn' | 'error';

const levelToConsole: Record<DebugLevel, (...args: any[]) => void> = {
  info: console.info.bind(console),
  warn: console.warn.bind(console),
  error: console.error.bind(console),
};

function readFlagFromWindow(): boolean {
  if (typeof window === 'undefined') {
    return false;
  }
  const globalOverride = (window as typeof window & Record<string, unknown>)[GLOBAL_FLAG];
  if (typeof globalOverride === 'boolean') {
    return globalOverride;
  }
  try {
    const search = new URLSearchParams(window.location.search);
    const queryValue = search.get(QUERY_FLAG);
    if (queryValue && ['1', 'true', 'on', 'yes'].includes(queryValue.toLowerCase())) {
      return true;
    }
  } catch (_) {
    // ignore query parsing issues
  }
  try {
    const stored = window.localStorage?.getItem(STORAGE_KEY);
    if (!stored) {
      return false;
    }
    return ['1', 'true', 'on', 'yes'].includes(stored.toLowerCase());
  } catch (_) {
    return false;
  }
}

export function isPrivateBeachDebugEnabled(): boolean {
  if (typeof window === 'undefined') {
    return process.env.NEXT_PUBLIC_PRIVATE_BEACH_DEBUG === '1';
  }
  return readFlagFromWindow();
}

export function debugLog(
  source: string,
  message: string,
  payload?: Record<string, unknown>,
  level: DebugLevel = 'info',
): void {
  if (!isPrivateBeachDebugEnabled()) {
    return;
  }
  const timestamp = new Date().toISOString();
  const prefix = `[private-beach][debug][${timestamp}][${source}] ${message}`;
  const writer = levelToConsole[level] ?? console.info;
  if (payload) {
    try {
      const json = JSON.stringify(payload, null, 2);
      writer(`${prefix} ${json}`);
    } catch (_) {
      writer(prefix, payload);
    }
    return;
  }
  writer(prefix);
}

export function debugStack(skipFrames = 0): string | undefined {
  if (!isPrivateBeachDebugEnabled()) {
    return undefined;
  }
  const err = new Error();
  if (!err.stack) {
    return undefined;
  }
  const lines = err.stack.split('\n');
  const filtered = lines.slice(1 + skipFrames);
  return filtered.join('\n');
}
