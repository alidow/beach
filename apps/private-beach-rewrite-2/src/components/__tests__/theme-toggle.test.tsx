import { describe, expect, it, beforeEach, afterEach, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ThemeToggleButton } from '../ThemeToggleButton';

// Minimal matchMedia mock for getInitialDark()
function mockMatchMedia(matches: boolean) {
  return vi.fn().mockImplementation((query: string) => ({
    matches,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
}

describe('ThemeToggleButton', () => {
  const originalMatchMedia = window.matchMedia;

  beforeEach(() => {
    document.documentElement.classList.remove('dark');
    window.localStorage.removeItem('pb-theme');
  });

  afterEach(() => {
    window.matchMedia = originalMatchMedia;
  });

  it('toggles dark class and persists preference', async () => {
    window.matchMedia = mockMatchMedia(false) as unknown as typeof originalMatchMedia;
    const user = userEvent.setup();
    render(<ThemeToggleButton />);
    // Button mounts after hydration
    const btn = await screen.findByRole('button', { name: /dark mode|light mode/i });
    expect(document.documentElement.classList.contains('dark')).toBe(false);
    await user.click(btn);
    expect(document.documentElement.classList.contains('dark')).toBe(true);
    expect(window.localStorage.getItem('pb-theme')).toBe('dark');
    await user.click(btn);
    expect(document.documentElement.classList.contains('dark')).toBe(false);
    expect(window.localStorage.getItem('pb-theme')).toBe('light');
  });
});

