import { defineConfig, devices } from '@playwright/test';

const baseURL = process.env.PRIVATE_BEACH_REWRITE_URL || 'http://localhost:3003';

let parsedBase: URL;
try {
  parsedBase = new URL(baseURL);
} catch {
  parsedBase = new URL('http://localhost:3003');
}

const port = parsedBase.port || '3003';
const webServerCommand = process.env.SKIP_PLAYWRIGHT_WEBSERVER
  ? undefined
  : `PRIVATE_BEACH_BYPASS_AUTH=${process.env.PRIVATE_BEACH_BYPASS_AUTH ?? '0'} PORT=${port} npm run dev -- --hostname 0.0.0.0 --port ${port}`;

export default defineConfig({
  testDir: './tests/e2e',
  timeout: 60_000,
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: process.env.CI ? 'dot' : 'list',
  use: {
    baseURL,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  webServer: webServerCommand
    ? {
        command: webServerCommand,
        url: baseURL,
        reuseExistingServer: !process.env.CI,
        timeout: 120_000,
      }
    : undefined,
});
