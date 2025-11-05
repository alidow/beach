import { defineConfig, defaultExclude } from 'vitest/config';
import path from 'path';

const isCI = process.env.CI === 'true';
const e2ePatterns = ['tests/e2e/**/*.spec.ts', 'tests/perf/**/*.spec.ts'];

export default defineConfig({
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./vitest.setup.ts'],
    exclude: [
      ...defaultExclude,
      'tests/**/*.pw.spec.ts',
      ...(isCI ? [] : e2ePatterns),
    ],
    css: false,
  },
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  esbuild: {
    jsx: 'automatic',
    jsxImportSource: 'react',
  },
});
