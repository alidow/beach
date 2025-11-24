import { expect, Page, test } from '@playwright/test';
import { clerk, clerkSetup } from '@clerk/testing/playwright';
import { execSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

type BootstrapInfo = { session_id: string; join_code: string };

const repoRoot = path.resolve(__dirname, '../../../..');
const baseUrl = (process.env.PRIVATE_BEACH_REWRITE_URL || 'http://localhost:3003').replace(/\/$/, '');
const shouldRun = process.env.RUN_PONG_LHS_SMOKE === '1';
const clerkUser = process.env.CLERK_USER || 'test@beach.sh';
const clerkPass = process.env.CLERK_PASS || 'h3llo Beach';
const managerToken =
  process.env.PRIVATE_BEACH_MANAGER_TOKEN || process.env.DEV_MANAGER_INSECURE_TOKEN || 'DEV-MANAGER-TOKEN';

function run(cmd: string) {
  execSync(cmd, {
    cwd: repoRoot,
    stdio: 'inherit',
    timeout: 10 * 60 * 1000,
    env: {
      ...process.env,
      DEV_ALLOW_INSECURE_MANAGER_TOKEN: '1',
      DEV_MANAGER_INSECURE_TOKEN: managerToken,
      PRIVATE_BEACH_MANAGER_TOKEN: managerToken,
      NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_TOKEN: managerToken,
      PRIVATE_BEACH_BYPASS_AUTH: '0',
      BEACH_SESSION_SERVER: 'http://beach-road:4132',
      PONG_WATCHDOG_INTERVAL: '10.0',
    },
  });
}

async function ensureSignedIn(page: Page) {
  await clerkSetup({
    secretKey: process.env.CLERK_SECRET_KEY,
    publishableKey: process.env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY,
  });
  await page.goto(`${baseUrl}/beaches`, { waitUntil: 'domcontentloaded' });
  await clerk.signIn({
    page,
    signInParams: { strategy: 'password', identifier: clerkUser, password: clerkPass },
  });
  await page.goto(`${baseUrl}/beaches`, { waitUntil: 'domcontentloaded' });
}

async function createBeachViaUi(page: Page): Promise<string> {
  const newButton = page.getByRole('button', { name: /new beach/i }).first();
  const forceButton = page.getByTestId('force-create-beach');

  try {
    await newButton.waitFor({ state: 'visible', timeout: 20_000 });
    await newButton.click();
    const modalHeading = page.getByRole('heading', { name: /create private beach/i });
    await modalHeading.waitFor({ state: 'visible', timeout: 10_000 });
    await page.getByRole('button', { name: /^Create$/i }).last().click();
    await page.waitForURL(/\/beaches\/[0-9a-fA-F-]+/, { timeout: 60_000 });
  } catch {
    await forceButton.waitFor({ state: 'visible', timeout: 10_000 });
    await forceButton.click();
    await page.waitForURL(/\/beaches\/[0-9a-fA-F-]+/, { timeout: 60_000 });
  }

  const url = page.url();
  const match = url.match(/beaches\/([0-9a-fA-F-]+)/);
  if (!match) throw new Error(`unable to parse beach id from ${url}`);
  return match[1];
}

function latestBootstrap(root = '/tmp/pong-stack'): BootstrapInfo | null {
  if (!fs.existsSync(root)) return null;
  const entries = fs
    .readdirSync(root, { withFileTypes: true })
    .filter((d) => d.isDirectory())
    .map((d) => {
      const full = path.join(root, d.name);
      return { full, mtime: fs.statSync(full).mtimeMs };
    })
    .sort((a, b) => b.mtime - a.mtime);
  for (const entry of entries) {
    const file = path.join(entry.full, 'bootstrap-lhs.json');
    if (fs.existsSync(file)) {
      const data = JSON.parse(fs.readFileSync(file, 'utf8')) as BootstrapInfo;
      if (data.session_id && data.join_code) return data;
    }
  }
  return null;
}

async function dragLhsTile(page: Page) {
  const catalog = page.getByText(/node catalog/i).first();
  if (!(await catalog.isVisible())) {
    await page.getByRole('button', { name: /catalog|nodes/i }).first().click().catch(() => {});
    await catalog.waitFor({ state: 'visible', timeout: 5_000 }).catch(() => {});
  }
  const item = page.getByText(/application/i).first();
  const canvas = page.locator('.react-flow__pane').first();
  await item.dragTo(canvas, { targetPosition: { x: 200, y: 200 } });
}

test.describe('pong lhs connect smoke', () => {
  test.skip(!shouldRun, 'Set RUN_PONG_LHS_SMOKE=1 to run lhs connect smoke');
  test.setTimeout(10 * 60 * 1000);

  test('create beach, start stack, connect lhs tile', async ({ page }) => {
    await page.context().addCookies([
      { name: 'pb-manager-token', value: managerToken, domain: 'localhost', path: '/' },
      { name: 'pb-auto-open-create', value: '1', domain: 'localhost', path: '/' },
    ]);

    await ensureSignedIn(page);
    const beachId = await createBeachViaUi(page);

    run(`direnv exec . ./apps/private-beach/demo/pong/tools/pong-stack.sh start ${beachId}`);

    const bootstrap = latestBootstrap();
    if (!bootstrap) {
      const dirs = fs.existsSync('/tmp')
        ? fs
            .readdirSync('/tmp', { withFileTypes: true })
            .filter((d) => d.isDirectory())
            .map((d) => d.name)
        : [];
      throw new Error(
        `missing bootstrap-lhs.json in /tmp/pong-stack; available /tmp dirs: ${dirs.join(',') || 'none'}`
      );
    }

    await page.goto(`${baseUrl}/beaches/${beachId}`, { waitUntil: 'domcontentloaded' });
    await page.getByText(/loading canvas/i).first().waitFor({ state: 'hidden', timeout: 60_000 }).catch(() => {});

    await dragLhsTile(page);
    const tile = page.locator('[data-testid^="rf__node-tile:"]').last();
    await tile.click();
    await tile.getByPlaceholder(/session id/i).fill(bootstrap.session_id);
    await tile.getByPlaceholder(/passcode|join code/i).fill(bootstrap.join_code);
    const connectBtn = tile.getByRole('button', { name: /connect/i }).first();
    await connectBtn.click({ trial: false });

    await expect(tile.getByText(/connected|connected!/i).first()).toBeVisible({ timeout: 30_000 });
  });
});
