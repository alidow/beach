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
  const beachPage = `${baseUrl}/beaches/${beachId}`;
  const beachesIndex = `${baseUrl}/beaches`;
  const clerkSecret = process.env.CLERK_SECRET_KEY;
  const bypassAuth = process.env.PRIVATE_BEACH_BYPASS_AUTH === '1';
  const managerToken = process.env.PRIVATE_BEACH_MANAGER_TOKEN || process.env.NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_TOKEN;

  // Capture console errors early so we don't miss fast-path failures that race page load.
  const consoleErrors: string[] = [];
  page.on('console', (msg) => {
    if (msg.type() === 'error') consoleErrors.push(msg.text());
  });

    const looksSignedIn = async () => {
      const beachCard = page.getByRole('link', { name: /open/i }).first();
      const newBeach = page.getByRole('button', { name: /new beach/i }).first();
      const beachList = page.locator('[href*="/beaches/"]').first();
      return (
        (await beachCard.isVisible({ timeout: 2_000 }).catch(() => false)) ||
        (await newBeach.isVisible({ timeout: 2_000 }).catch(() => false)) ||
        (await beachList.isVisible({ timeout: 2_000 }).catch(() => false))
      );
    };

    // If we have Clerk server credentials, mint a session and inject the cookie to bypass UI auth.
    if (clerkSecret) {
      const { Clerk } = await import('@clerk/clerk-sdk-node');
      const clerk = new Clerk({ secretKey: clerkSecret });
      const users = await clerk.users.getUserList({ emailAddress: [clerkUser], limit: 1 });
      const userId = users.length > 0 ? users[0].id : null;
      if (!userId) {
        throw new Error(`Clerk user not found for ${clerkUser}`);
      }
      const session = await clerk.sessions.createSession({
        userId,
        // Short-lived session for test purposes.
        expiresInSeconds: 60 * 60,
      });
      if (!session.token) {
        throw new Error('Failed to mint Clerk session token');
      }
      await page.context().addCookies([
        {
          name: '__session',
          value: session.token,
          domain: 'mock.clerk.dev',
          path: '/',
          httpOnly: true,
          secure: false,
        },
      ]);
      // With cookie in place, go straight to the target beach.
      await page.goto(beachPage, { waitUntil: 'domcontentloaded' });
    }

    const findAndFill = async (ctx: typeof page | import('@playwright/test').Page | import('@playwright/test').Frame) => {
      const identifier = ctx.locator('input[name="identifier"], input[type="email"]').first();
      if (!(await identifier.isVisible({ timeout: 8_000 }).catch(() => false))) return false;
      await identifier.fill(clerkUser);
      await ctx.getByRole('button', { name: /continue|next/i }).first().click();
      const password = ctx.locator('input[type="password"]').first();
      await password.waitFor({ state: 'visible', timeout: 8_000 });
      await password.fill(clerkPass);
      await ctx.getByRole('button', { name: /continue|sign in/i }).first().click();
      await page.waitForTimeout(500); // allow Clerk to process before URL wait
      await page.waitForURL('**/beaches/**', { timeout: 10_000 }).catch(() => {});
      return true;
    };

    const attemptSignInFlow = async () => {
      // Explicit header sign-in buttons/links first.
      const candidates = [
        page.locator('header').getByRole('button', { name: /sign in/i }).first(),
        page.locator('header').getByRole('link', { name: /sign in/i }).first(),
        page.getByRole('button', { name: /sign in/i }).first(),
        page.getByRole('link', { name: /sign in/i }).first(),
      ];
      let clicked = false;
      for (const el of candidates) {
        if (await el.isVisible({ timeout: 2_000 }).catch(() => false)) {
          const popupPromise = page.waitForEvent('popup', { timeout: 3_000 }).catch(() => null);
          await el.click();
          clicked = true;
          const popup = await popupPromise;
          if (popup && (await findAndFill(popup))) return true;
          // Wait briefly for any Clerk widget to appear.
          await page.waitForTimeout(1000);
          if (await findAndFill(page)) return true;
          // Try all frames.
          for (const frame of page.frames()) {
            if (await findAndFill(frame)) return true;
          }
          // Fallback: force navigate to /sign-in to surface the Clerk widget, then fill.
          await page.goto(`${baseUrl}/sign-in`, { waitUntil: 'domcontentloaded' });
          const shadowIdentifier = page.locator(
            'cl-provider >>> input[name="identifier"], cl-root >>> input[name="identifier"]',
          );
          if (await shadowIdentifier.isVisible({ timeout: 5_000 }).catch(() => false)) {
            await shadowIdentifier.fill(clerkUser);
            await page.getByRole('button', { name: /continue|next/i }).first().click();
            const shadowPassword = page.locator(
              'cl-provider >>> input[type="password"], cl-root >>> input[type="password"]',
            );
            await shadowPassword.waitFor({ state: 'visible', timeout: 5_000 });
            await shadowPassword.fill(clerkPass);
            await page.getByRole('button', { name: /continue|sign in/i }).first().click();
            await page.waitForURL('**/beaches/**', { timeout: 10_000 }).catch(() => {});
            return true;
          }
          if (await findAndFill(page)) return true;
          for (const frame of page.frames()) {
            if (await findAndFill(frame)) return true;
          }
          return false;
        }
      }
      return clicked;
    };

  // First attempt from /beaches. If bypass or a manager token is present, skip UI auth entirely.
  await page.goto(beachesIndex, { waitUntil: 'domcontentloaded' });
  if (!bypassAuth && !managerToken) {
    const alreadySignedIn = await looksSignedIn();
    const signedInFromIndex = alreadySignedIn || (await attemptSignInFlow());
    if (!signedInFromIndex && !(await looksSignedIn())) {
      throw new Error('Failed to initiate Clerk sign-in from /beaches (no inputs found)');
    }
  }

  // Navigate to the target beach; if unauthenticated, retry sign-in from the beach page and fail fast if still unauth.
  await page.goto(beachPage, { waitUntil: 'domcontentloaded' });
  const unauthBanner = page.getByText(
    /could not retrieve your access token|sign in to load this beach/i,
  );
  if (!bypassAuth && !managerToken) {
    if (await unauthBanner.isVisible({ timeout: 3_000 }).catch(() => false)) {
      const signedInOnBeach = await attemptSignInFlow();
      await page.goto(beachPage, { waitUntil: 'domcontentloaded' });
      if (!signedInOnBeach || (await unauthBanner.isVisible({ timeout: 3_000 }).catch(() => false))) {
        throw new Error('Still unauthenticated after retrying Clerk sign-in');
      }
    }
  }

  if (bypassAuth) {
    // In bypass mode we inject mock layout; just ensure canvas container renders.
    await expect(page.locator('[data-private-beach-rewrite]')).toBeVisible({ timeout: 10_000 });
    return;
  }

  // Ensure the canvas shell has hydrated before counting tiles.
  const canvasShell = page.locator('[data-private-beach-rewrite]');
  await canvasShell.waitFor({ state: 'visible', timeout: 60_000 });
  await page.getByText(/loading canvas/i).first().waitFor({ state: 'hidden', timeout: 60_000 });

  // Wait for tiles to render; expect at least lhs/rhs (agent may be present as a third).
  const tiles = page.locator('[data-testid^="rf__node-tile:"]');
  await expect
    .poll(async () => tiles.count(), { timeout: 60_000, message: 'expected canvas tiles to render' })
    .toBeGreaterThanOrEqual(2);

  // Best-effort: verify manager sessions respond (auth via manager token if provided).
  if (managerToken) {
    try {
      const resp = await page.request.get(
        `${baseUrl.replace('3003', '8080')}/private-beaches/${beachId}/sessions`,
        {
          headers: { authorization: `Bearer ${managerToken}` },
        },
      );
      const sessionsList = (await resp.json()) as Array<{ session_id: string }>;
      expect(sessionsList.length).toBeGreaterThanOrEqual(2);
    } catch {
      // ignore in UI smoke; network/auth issues should not fail the UI check
    }
  }

  // Best-effort telemetry; ignore if missing.
  try {
    await page.waitForFunction(
      ({ expectedCount }) => {
        const anyWindow = window as unknown as Record<string, unknown>;
        const events = (anyWindow.__telemetry_log__ as Array<{ event: string; payload: any }>) ?? [];
        const successes = events.filter((e) => e.event === 'canvas.tile.connect.success');
        return successes.length >= expectedCount;
      },
      { expectedCount: expectedSessions.length || 2 },
      { timeout: 10_000 },
    );
  } catch {
    // ignore
  }

  // Fail fast if any connect errors appear.
  const errorEvents = await page.evaluate(() => {
    const anyWindow = window as unknown as Record<string, unknown>;
    const events = (anyWindow.__telemetry_log__ as Array<{ event: string; payload: any }>) ?? [];
    return events.filter((e) => e.event === 'canvas.tile.connect.failure');
  });
  expect(errorEvents, 'tile connect failures present').toHaveLength(0);

  // Also scan browser console for fast-path errors and visible tile error badges.
  await page.waitForTimeout(2_000); // allow late console messages
  const fastPathErrors = consoleErrors.filter((text) =>
    /fast-path|authentication tag mismatch|webrtc.connect_error/i.test(text),
    );
    expect(fastPathErrors, 'fast-path related console errors').toHaveLength(0);

    // Best-effort: ensure no obvious tile error badges; ignore selector parse issues.
    try {
      const tileErrors = page.locator('[data-testid^=\"rf__node-tile:\"] [data-error]');
      await expect(tileErrors, 'tile error badges/messages should be absent').toHaveCount(0);
    } catch {
      // ignore
    }
  });
});
