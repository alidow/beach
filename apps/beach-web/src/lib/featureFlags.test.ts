import { beforeEach, describe, expect, it, vi } from 'vitest';
import { resolveAppVariant, shouldUseAppV2 } from './featureFlags';

describe('featureFlags', () => {
  beforeEach(() => {
    vi.unstubAllEnvs();
    vi.stubEnv('VITE_BEACH_WEB_UI', '');
    window.history.replaceState(null, '', '/');
  });

  it('defaults to legacy when no overrides present', () => {
    expect(resolveAppVariant()).toBe('legacy');
    expect(shouldUseAppV2()).toBe(false);
  });

  it('prefers query parameter over other sources', () => {
    window.history.replaceState(null, '', '/some/path?ui=v2');
    expect(resolveAppVariant()).toBe('v2');
  });

  it('falls back to env flag when query missing', () => {
    vi.stubEnv('VITE_BEACH_WEB_UI', 'v2');
    expect(resolveAppVariant()).toBe('v2');
  });

  it('detects v2 route prefix', () => {
    window.history.replaceState(null, '', '/v2/shell');
    expect(resolveAppVariant()).toBe('v2');
  });

  it('treats explicit legacy query as legacy even on v2 path', () => {
    window.history.replaceState(null, '', '/v2?ui=legacy');
    expect(resolveAppVariant()).toBe('legacy');
  });
});
