import { expect, test } from '@playwright/test';

const baseUrl = (process.env.PRIVATE_BEACH_REWRITE_URL || 'http://localhost:3003').replace(/\/$/, '');
const managerToken =
  process.env.PRIVATE_BEACH_MANAGER_TOKEN || process.env.DEV_MANAGER_INSECURE_TOKEN || 'DEV-MANAGER-TOKEN';
const shouldRun = process.env.RUN_BEACHES_SMOKE === '1';

test.describe('beaches smoke', () => {
  test.skip(!shouldRun, 'Set RUN_BEACHES_SMOKE=1 to run beaches smoke check');
  test.setTimeout(3 * 60 * 1000);

  test('renders create controls or surfaces error', async ({ page }, testInfo) => {
    const logs: string[] = [];
    page.on('console', (msg) => logs.push(`[console ${msg.type()}] ${msg.text()}`));
    page.on('pageerror', (err) => logs.push(`[pageerror] ${err.message}`));
    page.on('requestfailed', (req) => logs.push(`[requestfailed] ${req.url()} ${req.failure()?.errorText}`));

    await page.context().addCookies([
      { name: 'pb-manager-token', value: managerToken, domain: 'localhost', path: '/' },
      { name: 'pb-auto-open-create', value: '1', domain: 'localhost', path: '/' },
    ]);

    await page.goto(`${baseUrl}/beaches`, { waitUntil: 'domcontentloaded' });

    const newBeachButton = page.getByRole('button', { name: /new beach/i }).first();
    const forceCreate = page.getByTestId('force-create-beach');
    const errorBanner = page.getByText(/unable to load beaches|fetch failed|missing/i).first();

    const result = await Promise.race([
      newBeachButton.waitFor({ state: 'visible', timeout: 15_000 }).then(() => 'new-beach'),
      forceCreate.waitFor({ state: 'visible', timeout: 15_000 }).then(() => 'force-create'),
      errorBanner.waitFor({ state: 'visible', timeout: 15_000 }).then(() => 'error'),
    ]).catch(() => 'none');

    const screenshotPath = testInfo.outputPath('beaches-smoke.png');
    await page.screenshot({ path: screenshotPath, fullPage: true }).catch(() => {});

    // Log findings to stdout for quick debugging.
    console.log('[beaches-smoke] outcome', { result, logs });
    console.log(`[beaches-smoke] screenshot: ${screenshotPath}`);

    expect(result === 'new-beach' || result === 'force-create').toBeTruthy();
  });
});
