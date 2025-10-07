export type UiShellVariant = 'legacy' | 'terminal-first';

const V2_PATH_PREFIX = '/v2';
const SEARCH_PARAM_KEYS = ['ui', 'shell'];
const V2_PARAM_VALUES = new Set(['v2', 'terminal-first', 'terminal']);

export function resolveUiShellVariant(): UiShellVariant {
  if (typeof window === 'undefined') {
    return 'legacy';
  }
  const { pathname, search } = window.location;
  if (pathname.startsWith(V2_PATH_PREFIX)) {
    return 'terminal-first';
  }
  const params = new URLSearchParams(search);
  for (const key of SEARCH_PARAM_KEYS) {
    const value = params.get(key);
    if (value && V2_PARAM_VALUES.has(value.toLowerCase())) {
      return 'terminal-first';
    }
  }
  return 'legacy';
}

export function isTerminalFirstShell(): boolean {
  return resolveUiShellVariant() === 'terminal-first';
}
