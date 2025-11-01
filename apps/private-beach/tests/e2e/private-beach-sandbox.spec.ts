import { expect, test } from '@playwright/test';

const SESSION_ID = 'sandbox-session';

function buildSandboxUrl(): string {
  const params = new URLSearchParams({
    skipApi: '1',
    privateBeachId: 'sandbox',
    sessions: `${SESSION_ID}|application|Sandbox Fixture`,
    terminalFixtures: `${SESSION_ID}:pong-lhs`,
    viewerToken: 'sandbox-token',
    tileWidth: '448',
    tileHeight: '448',
  });
  return `/dev/private-beach-sandbox?${params.toString()}`;
}

test('Private Beach Sandbox renders terminal fixture and survives interaction', async ({ page }) => {
  await page.goto(buildSandboxUrl());

  // Wait for the tile header to appear so we know the layout mounted.
  const tile = page.getByTestId('rf__node-tile:sandbox-session');
  await expect(tile).toBeVisible();
  await expect(tile.getByRole('button', { name: 'Sandbox Fixture', exact: true })).toBeVisible();

  const placeholder = page.getByText('Preparing terminal preview…');
  await expect(placeholder).toHaveCount(0, { timeout: 30_000 });

  // The static fixture should render the marquee banner text.
  await expect(page.locator('body')).toContainText('PRIVATE BEACH PONG', { timeout: 30_000 });

  // Interact with the tile and confirm the text remains visible (no reconnect flash).
  await tile.getByRole('button', { name: 'Sandbox Fixture', exact: true }).click();
  await expect(page.locator('body')).toContainText('PRIVATE BEACH PONG');
});

test('viewer metrics telemetry updates after resize storm interactions', async ({ page }) => {
  await page.goto(buildSandboxUrl());

  const tile = page.getByTestId('rf__node-tile:sandbox-session');
  await expect(tile).toBeVisible();

  await page.evaluate(() => {
    const svc = (window as any).__PRIVATE_BEACH_VIEWER_SERVICE__;
    svc?.resetMetrics?.();
    const store = (window as any).__private_beach_viewer_counters__;
    if (store?.clear) {
      store.clear();
    }
    svc?.connectTile?.(
      'sandbox-session',
      {
        sessionId: '',
        privateBeachId: null,
        managerUrl: '',
        authToken: '',
      },
      () => {},
    );
  });

  const handle = page.locator('.react-resizable-handle-se').first();
  await expect(handle).toBeVisible();

  for (let i = 0; i < 5; i += 1) {
    const box = await handle.boundingBox();
    if (!box) break;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + 20, box.y + box.height / 2 + 20, { steps: 10 });
    await page.mouse.up();
  }

  await page.evaluate(() => {
    const svc = (window as any).__PRIVATE_BEACH_VIEWER_SERVICE__;
    if (!svc?.debugEmit) return;
    const makeSnapshot = (status: string, extra: Record<string, unknown> = {}) => ({
      store: null,
      transport: null,
      connecting: status === 'connecting' || status === 'reconnecting',
      error: null,
      status,
      secureSummary: null,
      latencyMs: null,
      transportVersion: 0,
      ...extra,
    });
    svc.debugEmit('sandbox-session', makeSnapshot('connecting', { connecting: true }));
    svc.debugEmit('sandbox-session', makeSnapshot('connected', { connecting: false, latencyMs: 48 }));
  });

  const counters = await page.evaluate(() => {
    const store = (window as any).__private_beach_viewer_counters__;
    if (!store || typeof store.entries !== 'function') {
      return [];
    }
    return Array.from(store.entries()).map(([tileId, value]: [string, any]) => ({
      tileId,
      counters: value,
    }));
  });

  const sandboxCounters = counters.find((entry) => entry.tileId === 'sandbox-session');
  expect(sandboxCounters?.counters.started ?? 0).toBeGreaterThanOrEqual(1);
  expect(sandboxCounters?.counters.completed ?? 0).toBeGreaterThanOrEqual(1);
  expect(sandboxCounters?.counters.retries ?? 0).toBe(0);
});

test('viewer metrics capture simulated reconnection telemetry', async ({ page }) => {
  await page.goto(buildSandboxUrl());

  await page.evaluate(() => {
    const svc = (window as any).__PRIVATE_BEACH_VIEWER_SERVICE__;
    svc?.resetMetrics?.();
    const store = (window as any).__private_beach_viewer_counters__;
    if (store?.clear) {
      store.clear();
    }
    svc?.connectTile?.(
      'sandbox-session',
      {
        sessionId: '',
        privateBeachId: null,
        managerUrl: '',
        authToken: '',
      },
      () => {},
    );
    if (!svc?.debugEmit) {
      return;
    }
    const makeSnapshot = (status: string, extra: Record<string, unknown> = {}) => ({
      store: null,
      transport: null,
      connecting: status === 'connecting' || status === 'reconnecting',
      error: null,
      status,
      secureSummary: null,
      latencyMs: null,
      transportVersion: 0,
      ...extra,
    });
    svc.debugEmit('sandbox-session', makeSnapshot('connecting', { connecting: true }));
    svc.debugEmit('sandbox-session', makeSnapshot('connected', { connecting: false, latencyMs: 60 }));
    svc.debugEmit('sandbox-session', makeSnapshot('reconnecting', { connecting: true }));
    svc.debugEmit(
      'sandbox-session',
      makeSnapshot('error', { connecting: false, error: 'keepalive failure during resize' }),
    );
    svc.debugEmit('sandbox-session', makeSnapshot('reconnecting', { connecting: true }));
    svc.debugEmit('sandbox-session', makeSnapshot('connected', { connecting: false, latencyMs: 72 }));
  });

  const counters = await page.evaluate(() => {
    const store = (window as any).__private_beach_viewer_counters__;
    if (!store || typeof store.entries !== 'function') {
      return [];
    }
    return Array.from(store.entries()).map(([tileId, value]: [string, any]) => ({
      tileId,
      counters: value,
    }));
  });

  const sandboxCounters = counters.find((entry) => entry.tileId === 'sandbox-session');
  expect(sandboxCounters?.counters.started ?? 0).toBeGreaterThanOrEqual(2);
  expect(sandboxCounters?.counters.retries ?? 0).toBeGreaterThanOrEqual(1);
  expect(sandboxCounters?.counters.failures ?? 0).toBeGreaterThanOrEqual(1);
});

test.describe('Controller-driven grid regression plan', () => {
  test.skip('persists grid geometry after drag/resize (post-Milestone 3)', async ({ page }) => {
    await page.goto(buildSandboxUrl());
    // TODO: implement once TileCanvas reads/writes controller view state.
  });

  test.skip('restores toolbar/lock status from controller metadata', async ({ page }) => {
    await page.goto(buildSandboxUrl());
    // TODO: implement verification once toolbar/lock state is controller-managed.
  });

  test.skip('applies autosize from controller measurement queue', async ({ page }) => {
    await page.goto(buildSandboxUrl());
    // TODO: simulate autosize trigger and assert controller snapshot drives layout.
  });
});
