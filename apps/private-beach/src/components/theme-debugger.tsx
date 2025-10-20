'use client';

import { useEffect } from 'react';
import { useTheme } from 'next-themes';

const isDev = process.env.NODE_ENV !== 'production';

export function ThemeDebugger() {
  const { theme, resolvedTheme, systemTheme } = useTheme();

  useEffect(() => {
    if (!isDev || typeof window === 'undefined') return;
    const docClass = document.documentElement.className;
    const stored = window.localStorage.getItem('pb-theme');
    const prefersDark = window.matchMedia?.('(prefers-color-scheme: dark)').matches;
    const computed = getComputedStyle(document.documentElement);
    const bg = computed.getPropertyValue('--background');
    const fg = computed.getPropertyValue('--foreground');
    // eslint-disable-next-line no-console
    console.debug('[theme]', {
      theme,
      resolvedTheme,
      systemTheme,
      docClass,
      stored,
      prefersDark,
      bg,
      fg,
    });
  }, [theme, resolvedTheme, systemTheme]);

  return null;
}
