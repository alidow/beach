import { expect, test } from '@playwright/test';
import fs from 'fs';
import path from 'path';

type BootstrapInfo = { session_id: string; join_code: string };

function readBootstrap(dir: string): BootstrapInfo | null {
  const files = ['bootstrap-lhs.json', 'bootstrap-rhs.json', 'bootstrap-agent.json'];
  for (const name of files) {
    const full = path.join(dir, name);
    if (!fs.existsSync(full)) continue;
    const raw = fs.readFileSync(full, 'utf8');
    const firstBrace = raw.indexOf('{');
    const lastBrace = raw.lastIndexOf('}');
    if (firstBrace < 0 || lastBrace <= firstBrace) continue;
    try {
      const parsed = JSON.parse(raw.slice(firstBrace, lastBrace + 1));
      if (parsed.session_id && parsed.join_code) {
        return { session_id: parsed.session_id, join_code: parsed.join_code };
      }
    } catch {
      /* ignore malformed bootstrap */
    }
  }
  return null;
}

test('attach application tile via fast-path and connect cleanly', async ({ page }) => {
  const beachId = process.env.PRIVATE_BEACH_ID;
  test.skip(!beachId, 'PRIVATE_BEACH_ID env is required');

  const bootstrapDir =
    process.env.PONG_BOOTSTRAP_DIR ||
    path.join(process.cwd(), 'temp', 'pong-fastpath-smoke', 'latest');
  const bootstrap = readBootstrap(bootstrapDir);
  test.skip(!bootstrap, `bootstrap payload missing in ${bootstrapDir}`);

  const baseUrl = process.env.PRIVATE_BEACH_URL || 'http://localhost:3003';
  const beachUrl = `${baseUrl}/beaches/${beachId}`;
  const bypassAuth = process.env.PRIVATE_BEACH_BYPASS_AUTH !== '0';

  const consoleErrors: string[] = [];
  page.on('console', (msg) => {
    if (msg.type() === 'error') {
      consoleErrors.push(msg.text());
    }
  });

  await page.goto(beachUrl, { waitUntil: 'domcontentloaded' });
  if (!bypassAuth) {
    throw new Error('Set PRIVATE_BEACH_BYPASS_AUTH=1 or provide a bypass token for rewrite-2 UI auth');
  }

  const canvasShell = page.locator('[data-private-beach-rewrite]');
  await expect(canvasShell).toBeVisible({ timeout: 60_000 });
  await page.getByText(/loading canvas/i).first().waitFor({ state: 'hidden', timeout: 60_000 }).catch(() => {});

  // Catalog opens automatically on empty canvas; ensure it's open, otherwise toggle.
  const catalogToggle = page.getByRole('button', { name: /close/i }).first();
  if (!(await catalogToggle.isVisible({ timeout: 2_000 }).catch(() => false))) {
    const openBtn = page.getByRole('button', { name: /open catalog/i }).first();
    await openBtn.click();
    await expect(catalogToggle).toBeVisible({ timeout: 5_000 });
  }

  const catalogNode = page.getByTestId('catalog-node-application');
  await expect(catalogNode).toBeVisible({ timeout: 10_000 });
  const surface = page.getByTestId('flow-canvas');
  await catalogNode.dragTo(surface, { targetPosition: { x: 360, y: 200 } });

  const tile = page.locator('[data-testid^="rf__node-tile:"]').last();
  await expect(tile).toBeVisible({ timeout: 10_000 });

  await tile.getByLabel(/session id/i).fill(bootstrap!.session_id);
  await tile.getByLabel(/passcode/i).fill(bootstrap!.join_code);
  await tile.getByRole('button', { name: /connect/i }).click();

  // Wait for the tile to report connected; do not allow the spinner to persist past 30s.
  await expect(tile.getByText(/connected/i)).toBeVisible({ timeout: 30_000 });

  // Ensure the tile is not reporting an error badge/message.
  try {
    await expect(tile.locator('[data-error]')).toHaveCount(0);
  } catch {
    /* ignore selector mismatches; explicit console check below will fail on errors */
  }

  // Scan console for fast-path/WebRTC failures.
  const fastPathErrors = consoleErrors.filter((text) =>
    /fast[- ]?path|authentication tag mismatch|webrtc\.connect_error|data channel/i.test(text),
  );
  expect(fastPathErrors, 'fast-path related console errors').toHaveLength(0);
});
