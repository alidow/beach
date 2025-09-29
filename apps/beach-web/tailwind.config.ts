import type { Config } from 'tailwindcss';
import { fontFamily } from 'tailwindcss/defaultTheme';
import animate from 'tailwindcss-animate';

const config: Config = {
  darkMode: ['class'],
  content: ['./index.html', './src/**/*.{ts,tsx,js,jsx}'],
  theme: {
    container: {
      center: true,
      padding: '1.5rem',
      screens: {
        '2xl': '1200px',
      },
    },
    extend: {
      colors: {
        border: 'hsl(var(--border))',
        input: 'hsl(var(--input))',
        ring: 'hsl(var(--ring))',
        background: 'hsl(var(--background))',
        foreground: 'hsl(var(--foreground))',
        muted: {
          DEFAULT: 'hsl(var(--muted))',
          foreground: 'hsl(var(--muted-foreground))',
        },
        popover: {
          DEFAULT: 'hsl(var(--popover))',
          foreground: 'hsl(var(--popover-foreground))',
        },
        card: {
          DEFAULT: 'hsl(var(--card))',
          foreground: 'hsl(var(--card-foreground))',
        },
        terminal: {
          frame: 'hsl(var(--terminal-frame))',
          bezel: 'hsl(var(--terminal-bezel))',
          screen: 'hsl(var(--terminal-screen))',
          glow: 'hsl(var(--terminal-glow))',
        },
        accent: {
          DEFAULT: 'hsl(var(--accent))',
          foreground: 'hsl(var(--accent-foreground))',
        },
        destructive: {
          DEFAULT: 'hsl(var(--destructive))',
          foreground: 'hsl(var(--destructive-foreground))',
        },
        success: {
          DEFAULT: 'hsl(var(--success))',
          foreground: 'hsl(var(--success-foreground))',
        },
        borderglow: 'hsl(var(--border-glow))',
      },
      borderRadius: {
        lg: 'var(--radius)',
        md: 'calc(var(--radius) - 2px)',
        sm: 'calc(var(--radius) - 4px)',
      },
      fontFamily: {
        sans: ['var(--font-sans)', ...fontFamily.sans],
        mono: ['var(--font-mono)', ...fontFamily.mono],
      },
      boxShadow: {
        'terminal-ring': '0 0 0 1px hsl(var(--border) / 0.4), 0 0 0 4px hsl(var(--border-glow) / 0.15)',
        'terminal-glow': '0 20px 60px -20px hsl(var(--terminal-glow))',
      },
      backgroundImage: {
        'terminal-gradient': 'radial-gradient(circle at top, hsl(var(--terminal-glow)) 0%, transparent 55%)',
      },
      keyframes: {
        'caret-blink': {
          '0%, 40%': { opacity: '1' },
          '40.01%, 100%': { opacity: '0' },
        },
        'slow-pulse': {
          '0%, 100%': { opacity: '1' },
          '50%': { opacity: '.6' },
        },
      },
      animation: {
        'caret-blink': 'caret-blink 1.1s steps(2, start) infinite',
        'slow-pulse': 'slow-pulse 4s ease-in-out infinite',
      },
    },
  },
  plugins: [animate],
};

export default config;
