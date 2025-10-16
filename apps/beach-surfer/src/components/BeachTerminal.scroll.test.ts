import { describe, expect, it } from 'vitest';
import { shouldReenableFollowTail } from './BeachTerminal';

describe('shouldReenableFollowTail', () => {
  it('treats the viewport as at the tail once no meaningful space remains', () => {
    expect(shouldReenableFollowTail(0)).toBe(true);
    expect(shouldReenableFollowTail(0.4)).toBe(true);
    expect(shouldReenableFollowTail(1)).toBe(true);
  });

  it('keeps manual scroll mode when there is still visible slack', () => {
    expect(shouldReenableFollowTail(1.0001)).toBe(false);
    expect(shouldReenableFollowTail(5)).toBe(false);
  });
});
