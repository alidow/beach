import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(process.env.npm_package_version ?? '0.0.1'),
  },
  assetsInclude: ['**/*.wasm'],
  test: {
    environment: 'jsdom',
    setupFiles: ['./vitest.setup.ts'],
    coverage: {
      provider: 'v8',
    },
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
    exclude: ['tests/**/*', 'playwright.config.ts'],
  },
});
