import { describe, expect, it } from 'vitest';

import { computeDimensionUpdate } from '../terminalHostDimensions';

describe('computeDimensionUpdate', () => {
  it('prefers viewport fallback before PTY data arrives', () => {
    const result = computeDimensionUpdate(null, null, 24, 'unknown');
    expect(result).toEqual({ value: 24, source: 'fallback', changed: true });
  });

  it('upgrades to PTY dimensions when host rows are reported', () => {
    const initial = computeDimensionUpdate(null, null, 24, 'unknown');
    expect(initial.value).toBe(24);
    expect(initial.source).toBe('fallback');

    const hostUpdate = computeDimensionUpdate(initial.value, 62, 24, initial.source);
    expect(hostUpdate.value).toBe(62);
    expect(hostUpdate.source).toBe('pty');
    expect(hostUpdate.changed).toBe(true);
  });

  it('allows fallback to grow but not shrink when host data is missing', () => {
    const first = computeDimensionUpdate(null, null, 24, 'unknown');
    expect(first.value).toBe(24);
    const grow = computeDimensionUpdate(first.value, null, 40, first.source);
    expect(grow.value).toBe(40);
    const shrink = computeDimensionUpdate(grow.value, null, 30, grow.source);
    expect(shrink.value).toBe(40);
    expect(shrink.changed).toBe(false);
  });

  it('preserves PTY dimensions once established even if fallback data changes', () => {
    const afterHost = { value: 62, source: 'pty' as const };

    const fallbackUpdate = computeDimensionUpdate(afterHost.value, null, 40, afterHost.source);
    expect(fallbackUpdate.value).toBe(62);
    expect(fallbackUpdate.source).toBe('pty');
    expect(fallbackUpdate.changed).toBe(false);
  });

  it('reflects updated PTY dimensions when they change', () => {
    const current = { value: 62, source: 'pty' as const };

    const next = computeDimensionUpdate(current.value, 72, 24, current.source);
    expect(next.value).toBe(72);
    expect(next.source).toBe('pty');
    expect(next.changed).toBe(true);
  });
});
