'use client';

import { useEffect, useState } from 'react';
import { Moon, Sun } from 'lucide-react';

function getInitialDark(): boolean {
  if (typeof window === 'undefined') return false;
  const stored = window.localStorage.getItem('pb-theme');
  if (stored === 'light') return false;
  if (stored === 'dark') return true;
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? false;
}

export function ThemeToggleButton() {
  const [mounted, setMounted] = useState(false);
  const [isDark, setIsDark] = useState(false);

  useEffect(() => {
    setMounted(true);
    setIsDark(getInitialDark());
  }, []);

  if (!mounted) return null;

  const handleToggle = () => {
    const next = !isDark;
    setIsDark(next);
    if (typeof document !== 'undefined') {
      const root = document.documentElement;
      if (next) {
        root.classList.add('dark');
      } else {
        root.classList.remove('dark');
      }
    }
    try {
      window.localStorage.setItem('pb-theme', next ? 'dark' : 'light');
    } catch {
      // ignore storage errors
    }
  };

  return (
    <button
      type="button"
      aria-label={isDark ? 'Switch to light mode' : 'Switch to dark mode'}
      title={isDark ? 'Light mode' : 'Dark mode'}
      onClick={handleToggle}
      className="inline-flex h-8 w-8 items-center justify-center rounded-full border border-white/10 bg-white/5 text-slate-300 transition hover:border-white/25 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-slate-300/40 dark:focus-visible:ring-slate-600/40"
    >
      {isDark ? <Sun size={16} /> : <Moon size={16} />}
      <span className="sr-only">Toggle theme</span>
    </button>
  );
}
