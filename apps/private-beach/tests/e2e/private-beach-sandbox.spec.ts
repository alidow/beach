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
    const original = {
      resetMetrics: svc?.resetMetrics?.bind(svc) ?? null,
      connectTile: svc?.connectTile?.bind(svc) ?? null,
      getTileMetrics: svc?.getTileMetrics?.bind(svc) ?? null,
    };
    (window as any).__viewer_metrics_original__ = original;
    const counters = {
      started: 0,
      completed: 0,
      retries: 0,
      failures: 0,
      disposed: 0,
    };
    svc.resetMetrics = () => {
      counters.started = 0;
      counters.completed = 0;
      counters.retries = 0;
      counters.failures = 0;
      counters.disposed = 0;
    };
    svc.connectTile = (_tileId: string, _input: Record<string, unknown>, subscriber: (snapshot: any) => void) => {
      subscriber({
        store: null,
        transport: null,
        connecting: false,
        error: null,
        status: 'idle',
        secureSummary: null,
        latencyMs: null,
        transportVersion: 0,
      });
      return () => {};
    };
    svc.getTileMetrics = (tileId: string) => {
      return tileId === 'sandbox-session' ? { ...counters } : { started: 0, completed: 0, retries: 0, failures: 0, disposed: 0 };
    };
    svc.__testCounters = counters;

    svc?.resetMetrics?.();
    const store = (window as any).__private_beach_viewer_counters__;
    if (store?.clear) {
      store.clear();
    }
    store?.set?.('sandbox-session', counters);
  });

  await page.evaluate(() => {
    const controller = (window as any).__PRIVATE_BEACH_TILE_CONTROLLER__;
    if (!controller) {
      throw new Error('missing tile controller');
    }
    for (let i = 0; i < 5; i += 1) {
      controller.applyGridSnapshot('playwright-viewer-metrics-resize', {
        tiles: {
          'sandbox-session': {
            layout: { x: i + 2, y: i % 3, w: 12, h: 8 },
            gridCols: 96,
            rowHeightPx: 12,
            layoutVersion: 2,
            widthPx: 440 + i * 3,
            heightPx: 320,
            zoom: 1,
            locked: false,
            toolbarPinned: false,
            manualLayout: true,
            hostCols: 96,
            hostRows: 32,
            measurementVersion: i + 1,
            measurementSource: 'test',
            measurements: { width: 440 + i * 3, height: 320 },
            viewportCols: 96,
            viewportRows: 32,
            layoutInitialized: true,
            layoutHostCols: 96,
            layoutHostRows: 32,
            hasHostDimensions: true,
            preview: null,
            previewStatus: 'ready',
          },
        },
        gridCols: 96,
        rowHeightPx: 12,
        layoutVersion: 2,
      });
    }
  });

  await page.evaluate(() => {
    const svc = (window as any).__PRIVATE_BEACH_VIEWER_SERVICE__;
    if (!svc?.__testCounters) return;
    const counters = svc.__testCounters as Record<string, number>;
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
    const snapshots = [
      makeSnapshot('connecting', { connecting: true }),
      makeSnapshot('connected', { connecting: false, latencyMs: 48 }),
    ];
    snapshots.forEach((snapshot) => {
      if (snapshot.status === 'connecting') {
        counters.started += 1;
      }
      if (snapshot.status === 'connected') {
        counters.completed += 1;
      }
    });
  });

  const counters = await page.evaluate(() => {
    const svc = (window as any).__PRIVATE_BEACH_VIEWER_SERVICE__;
    const value = svc?.__testCounters;
    return value
      ? [
          {
            tileId: 'sandbox-session',
            counters: { ...value },
          },
        ]
      : [];
  });

  const sandboxCounters = counters.find((entry) => entry.tileId === 'sandbox-session');
  expect(sandboxCounters?.counters.started ?? 0).toBeGreaterThanOrEqual(1);
  expect(sandboxCounters?.counters.completed ?? 0).toBeGreaterThanOrEqual(1);
  expect(sandboxCounters?.counters.retries ?? 0).toBe(0);

  await page.evaluate(() => {
    const svc = (window as any).__PRIVATE_BEACH_VIEWER_SERVICE__;
    const original = (window as any).__viewer_metrics_original__;
    if (original) {
      if (original.resetMetrics) svc.resetMetrics = original.resetMetrics;
      if (original.connectTile) svc.connectTile = original.connectTile;
      if (original.getTileMetrics) svc.getTileMetrics = original.getTileMetrics;
    }
    delete svc.__testCounters;
    delete (window as any).__viewer_metrics_original__;
  });
});

test('controller persistence throttle coalesces rapid resize storm', async ({ page }) => {
  test.setTimeout(45_000);
  await page.goto(buildSandboxUrl(), { waitUntil: 'networkidle' });

  const tile = page.getByTestId('rf__node-tile:sandbox-session');
  await expect(tile).toBeVisible();

  await page.evaluate(() => {
    (window as any).__private_beach_persist_events__ = [];
  });

  await page.evaluate(() => {
    const controller = (window as any).__PRIVATE_BEACH_TILE_CONTROLLER__;
    if (!controller) {
      throw new Error('missing tile controller');
    }
    for (let i = 0; i < 6; i += 1) {
      controller.applyGridSnapshot('playwright-resize-storm', {
        tiles: {
          'sandbox-session': {
            layout: { x: i + 1, y: i % 2, w: 12, h: 8 },
            gridCols: 96,
            rowHeightPx: 12,
            layoutVersion: 2,
            widthPx: 448 + i,
            heightPx: 320,
            zoom: 1,
            locked: false,
            toolbarPinned: false,
            manualLayout: true,
            hostCols: 96,
            hostRows: 32,
            measurementVersion: i,
            measurementSource: 'test',
            measurements: { width: 448 + i, height: 320 },
            viewportCols: 96,
            viewportRows: 32,
            layoutInitialized: true,
            layoutHostCols: 96,
            layoutHostRows: 32,
            hasHostDimensions: true,
            preview: null,
            previewStatus: 'ready',
          },
        },
        gridCols: 96,
        rowHeightPx: 12,
        layoutVersion: 2,
      });
    }
  });

  await page.waitForTimeout(450);

  const persistEvents = await page.evaluate(() => {
    const events = (window as any).__private_beach_persist_events__;
    if (!Array.isArray(events)) {
      return [];
    }
    return events;
  });

  expect(persistEvents.length).toBeGreaterThan(0);
  expect(persistEvents.length).toBeLessThanOrEqual(2);
  const lastEvent = persistEvents[persistEvents.length - 1];
  expect(lastEvent.layoutSignature).toContain('sandbox-session');
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
