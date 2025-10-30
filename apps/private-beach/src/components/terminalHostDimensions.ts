'use client';

export type HostDimensionSource = 'unknown' | 'pty' | 'fallback';

export function computeDimensionUpdate(
  currentValue: number | null,
  hostValue: number | null | undefined,
  fallbackValue: number | null | undefined,
  currentSource: HostDimensionSource,
): { value: number | null; source: HostDimensionSource; changed: boolean } {
  const normalizedHost = typeof hostValue === 'number' && hostValue > 0 ? hostValue : null;
  const normalizedFallback = typeof fallbackValue === 'number' && fallbackValue > 0 ? fallbackValue : null;

  if (normalizedHost != null) {
    const changed = currentSource !== 'pty' || currentValue !== normalizedHost;
    return { value: normalizedHost, source: 'pty', changed };
  }

  if (currentSource === 'pty') {
    return { value: currentValue, source: 'pty', changed: false };
  }

  if (normalizedFallback != null) {
    const changed = currentSource !== 'fallback' || currentValue !== normalizedFallback;
    return { value: normalizedFallback, source: 'fallback', changed };
  }

  if (currentSource === 'fallback') {
    return { value: currentValue, source: 'fallback', changed: false };
  }

  if (currentValue != null) {
    return { value: currentValue, source: currentSource, changed: false };
  }

  return { value: null, source: 'unknown', changed: false };
}
