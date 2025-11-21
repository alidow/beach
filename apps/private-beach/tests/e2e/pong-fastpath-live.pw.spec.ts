import { expect, test } from '@playwright/test';
import fs from 'fs';
import path from 'path';

type BootstrapInfo = { session_id: string; join_code: string };

function loadBootstrapSessions(dir: string): BootstrapInfo[] {
  const files = ['bootstrap-lhs.json', 'bootstrap-rhs.json', 'bootstrap-agent.json'];
  const sessions: BootstrapInfo[] = [];
  for (const file of files) {
    const full = path.join(dir, file);
    if (!fs.existsSync(full)) continue;
    try {
      const raw = fs.readFileSync(full, 'utf8');
      // Some bootstrap files are prefixed with build output; parse between the first '{' and last '}'.
      const firstBrace = raw.indexOf('{');
      const lastBrace = raw.lastIndexOf('}');
      if (firstBrace < 0 || lastBrace <= firstBrace) continue;
      const parsed = JSON.parse(raw.slice(firstBrace, lastBrace + 1));
      if (parsed.session_id && parsed.join_code) {
        sessions.push({ session_id: parsed.session_id, join_code: parsed.join_code });
      }
    } catch (err) {
      // Ignore malformed files; test will rely on telemetry fallback.
      console.warn(`failed to parse ${full}:`, err);
      continue;
    }
  }
  return sessions;
}

test.describe('pong fast-path live (rewrite-2)', () => {
  const beachId = process.env.PRIVATE_BEACH_ID;
  const baseUrl = process.env.PRIVATE_BEACH_URL || 'http://localhost:3003';
  const bootstrapDir =
    process.env.PONG_BOOTSTRAP_DIR ||
    path.join(process.cwd(), 'temp', 'pong-fastpath-smoke', 'latest');

  const expectedSessions = loadBootstrapSessions(bootstrapDir);

  test.beforeEach(({ page }) => {
    // Capture telemetry events emitted by rewrite-2 frontend.
    page.addInitScript(() => {
      const anyWindow = window as unknown as Record<string, unknown>;
      const events: Array<{ event: string; payload: any }> = [];
      anyWindow.__telemetry_log__ = events;
      anyWindow.__BEACH_TELEMETRY__ = (event: string, payload: any) => {
        events.push({ event, payload });
      };
    });
  });

  test('tiles connect without fast-path errors', async ({ page }) => {
    test.skip(!beachId, 'PRIVATE_BEACH_ID env is required for live pong fast-path test');

    const clerkUser = process.env.CLERK_USER || 'test@beach.sh';
    const clerkPass = process.env.CLERK_PASS || 'h3llo Beach';

    // Navigate to sign-in first to ensure Clerk loads.
    await page.goto(`${baseUrl}/sign-in`, { waitUntil: 'domcontentloaded' });

    // If already signed in, redirect may land on /beaches; otherwise, complete Clerk form.
    const identifier = page.locator('input[name="identifier"], input[type="email"]').first();
    if (await identifier.isVisible({ timeout: 3_000 }).catch(() => false)) {
      await identifier.fill(clerkUser);
      await page.getByRole('button', { name: /continue|next/i }).first().click();
      const password = page.locator('input[type="password"]').first();
      await password.waitFor({ state: 'visible', timeout: 15_000 });
      await password.fill(clerkPass);
      await page.getByRole('button', { name: /continue|sign in/i }).first().click();
    }

    // Hit the target beach page after auth.
    await page.goto(`${baseUrl}/beaches/${beachId}`, { waitUntil: 'domcontentloaded' });
    await page.waitForURL('**/beaches/**', { timeout: 20_000 }).catch(() => {});

    // Wait for tiles to render; expect at least 3.
    const tiles = page.locator('[data-testid^="rf__node-tile:"]');
    await expect(tiles).toHaveCount(3, { timeout: 30_000 });

    // Wait for at least one connect success event per expected session (or at least 3 total).
    await page.waitForFunction(
      ({ expectedCount }) => {
        const anyWindow = window as unknown as Record<string, unknown>;
        const events = (anyWindow.__telemetry_log__ as Array<{ event: string; payload: any }>) ?? [];
        const successes = events.filter((e) => e.event === 'canvas.tile.connect.success');
        return successes.length >= expectedCount;
      },
      { expectedCount: expectedSessions.length || 3 },
      { timeout: 30_000 },
    );

    // Fail fast if any connect errors appear.
    const errorEvents = await page.evaluate(() => {
      const anyWindow = window as unknown as Record<string, unknown>;
      const events = (anyWindow.__telemetry_log__ as Array<{ event: string; payload: any }>) ?? [];
      return events.filter((e) => e.event === 'canvas.tile.connect.failure');
    });
    expect(errorEvents, 'tile connect failures present').toHaveLength(0);

    const successEvents = await page.evaluate(() => {
      const anyWindow = window as unknown as Record<string, unknown>;
      const events = (anyWindow.__telemetry_log__ as Array<{ event: string; payload: any }>) ?? [];
      return events.filter((e) => e.event === 'canvas.tile.connect.success');
    });

    if (expectedSessions.length > 0) {
      const connectedIds = successEvents.map((e: any) => e.payload?.sessionId).filter(Boolean);
      for (const sess of expectedSessions) {
        expect(
          connectedIds,
          `expected session ${sess.session_id} to connect via canvas telemetry`,
        ).toContain(sess.session_id);
      }
    }

    // Also scan browser console for fast-path errors.
    const consoleErrors: string[] = [];
    page.on('console', (msg) => {
      if (msg.type() === 'error') consoleErrors.push(msg.text());
    });
    await page.waitForTimeout(2_000); // allow late console messages
    const fastPathErrors = consoleErrors.filter((text) =>
      /fast-path|authentication tag mismatch|webrtc.connect_error/i.test(text),
    );
    expect(fastPathErrors, 'fast-path related console errors').toHaveLength(0);
  });
});
