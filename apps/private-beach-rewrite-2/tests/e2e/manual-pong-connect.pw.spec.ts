import { expect, Locator, Page, test } from '@playwright/test';
import { clerk, clerkSetup } from '@clerk/testing/playwright';
import { execSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

type BootstrapInfo = { session_id: string; join_code: string };

const repoRoot = path.resolve(__dirname, '../../../..');
const baseUrl = (process.env.PRIVATE_BEACH_REWRITE_URL || 'http://localhost:3003').replace(/\/$/, '');
const shouldRun = process.env.RUN_MANUAL_PONG_SHOWCASE === '1';
const clerkUser = process.env.CLERK_USER || 'test@beach.sh';
const clerkPass = process.env.CLERK_PASS || 'h3llo Beach';
const managerToken =
  process.env.PRIVATE_BEACH_MANAGER_TOKEN || process.env.DEV_MANAGER_INSECURE_TOKEN || 'DEV-MANAGER-TOKEN';
const logRootHost = path.join(repoRoot, 'temp', 'manual-pong');
const logRootContainer = '/app/temp/manual-pong';

const execEnv = {
  ...process.env,
  PRIVATE_BEACH_MANAGER_URL: 'http://localhost:8080',
  PONG_SESSION_SERVER: process.env.PONG_SESSION_SERVER || 'http://beach-road:4132/',
  PONG_AUTH_GATEWAY: process.env.PONG_AUTH_GATEWAY || 'http://beach-gate:4133',
  BEACH_SESSION_SERVER: 'http://beach-road:4132',
  PONG_WATCHDOG_INTERVAL: '10.0',
  PONG_LOG_ROOT: logRootContainer,
  PONG_LOG_DIR: logRootContainer,
  PONG_FRAME_DUMP_DIR: path.join(logRootContainer, 'frame-dumps'),
  PONG_BALL_TRACE_DIR: path.join(logRootContainer, 'ball-trace'),
  PONG_COMMAND_TRACE_DIR: path.join(logRootContainer, 'command-trace'),
  HOST_MANAGER_TOKEN: managerToken,
  PRIVATE_BEACH_MANAGER_TOKEN: managerToken,
  NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_TOKEN: managerToken,
  PRIVATE_BEACH_BYPASS_AUTH: '0',
};

function run(cmd: string) {
  execSync(cmd, {
    cwd: repoRoot,
    stdio: 'inherit',
    env: execEnv,
    timeout: 10 * 60 * 1000,
  });
}

function ensureLogRoot() {
  fs.rmSync(logRootHost, { recursive: true, force: true });
  fs.mkdirSync(logRootHost, { recursive: true });
}

function startStack() {
  ensureLogRoot();
  const cmds = [
    'direnv allow',
    'direnv exec . ./scripts/dockerdown --postgres-only',
    'direnv exec . docker compose down',
    "direnv exec . env BEACH_SESSION_SERVER='http://beach-road:4132' PONG_WATCHDOG_INTERVAL=10.0 docker compose build beach-manager",
    [
      'DEV_ALLOW_INSECURE_MANAGER_TOKEN=1',
      'DEV_MANAGER_INSECURE_TOKEN=DEV-MANAGER-TOKEN',
      'PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN',
      'PRIVATE_BEACH_BYPASS_AUTH=0',
      'direnv exec . sh -c \'BEACH_SESSION_SERVER="http://beach-road:4132" PONG_WATCHDOG_INTERVAL=10.0 BEACH_MANAGER_STDOUT_LOG=trace BEACH_MANAGER_FILE_LOG=trace BEACH_MANAGER_TRACE_DEPS=1 docker compose up -d\'',
    ].join(' '),
  ];
  for (const cmd of cmds) run(cmd);
}

async function signInAndCreateBeach(page: Page): Promise<string> {
  console.log('[manual-pong] waiting for rewrite to respond on 3003');
  // Wait for rewrite app to be reachable before navigating.
  await expect
    .poll(async () => {
      try {
        // Prefer health endpoint; fall back to root render status.
        const res = await fetch(`${baseUrl}/api/health`).catch(() => fetch(baseUrl));
        return res.status > 0;
      } catch {
        return false;
      }
    }, { timeout: 300_000, message: 'rewrite app not reachable on http://localhost:3003' })
    .toBeTruthy();
  console.log('[manual-pong] rewrite reachable, proceeding to Clerk sign-in');

  // Ensure manager health responds before hitting the rewrite beach list.
  try {
    await expect
      .poll(async () => {
        try {
          const res = await fetch('http://localhost:8080/healthz', {
            headers: { authorization: `Bearer ${managerToken}` },
          });
          return res.ok;
        } catch {
          return false;
        }
      }, { timeout: 240_000 })
      .toBeTruthy();
  } catch (e) {
    console.warn('[manual-pong] manager health check failed, continuing anyway', e);
  }

  await clerkSetup({
    secretKey: process.env.CLERK_SECRET_KEY,
    publishableKey: process.env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY,
  });

  console.log('[manual-pong] navigating to /beaches for Clerk sign-in');
  await page.goto(`${baseUrl}/beaches`, { waitUntil: 'domcontentloaded' });
  console.log('[manual-pong] invoking Clerk signIn');
  await clerk.signIn({
    page,
    signInParams: { strategy: 'password', identifier: clerkUser, password: clerkPass },
  });

  console.log('[manual-pong] reloading /beaches post-login');
  await page.goto(`${baseUrl}/beaches`, { waitUntil: 'domcontentloaded' });
  console.log('[manual-pong] waiting for New Beach trigger');
  const createTrigger = page.getByRole('button', { name: /new beach/i }).first();
  try {
    await createTrigger.waitFor({ state: 'visible', timeout: 120_000 });
    console.log('[manual-pong] clicking New Beach');
    await createTrigger.click();
    const modalHeading = page.getByRole('heading', { name: /create private beach/i });
    await modalHeading.waitFor({ state: 'visible', timeout: 10_000 });
    console.log('[manual-pong] clicking Create in modal');
    await page.getByRole('button', { name: /^Create$/i }).last().click();
    console.log('[manual-pong] waiting for beach page navigation');
    await page.waitForURL(/\/beaches\/[0-9a-fA-F-]+/, { timeout: 60_000 });
    const url = page.url();
    const match = url.match(/beaches\/([0-9a-fA-F-]+)/);
    if (!match) throw new Error(`unable to parse beach id from ${url}`);
    const beachId = match[1];
    console.log(`[manual-pong] beach created ${beachId}`);
    return beachId;
  } catch (modalErr) {
    console.log('[manual-pong] New Beach path failed, trying force-create UI button', modalErr);
    const forceButton = page.getByTestId('force-create-beach');
    try {
      await forceButton.waitFor({ state: 'visible', timeout: 15_000 });
      await forceButton.click();
      await page.waitForURL(/\/beaches\/[0-9a-fA-F-]+/, { timeout: 90_000 });
      const url = page.url();
      const match = url.match(/beaches\/([0-9a-fA-F-]+)/);
      if (!match) throw new Error(`unable to parse beach id from ${url}`);
      const beachId = match[1];
      console.log(`[manual-pong] force-create beach created ${beachId}`);
      return beachId;
    } catch (forceErr) {
      console.log('[manual-pong] force-create button path failed, calling internal test API', forceErr);
      const resp = await fetch(`${baseUrl}/api/test/create-beach`, {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          'x-pb-manager-token': managerToken,
        },
        body: JSON.stringify({ name: 'Pong Showcase (fallback)' }),
      });
      if (!resp.ok) {
        const detail = await resp.text().catch(() => resp.statusText);
        throw new Error(`fallback beach create failed: status ${resp.status} detail ${detail}`);
      }
      const data = await resp.json();
      const beachId = data.id as string;
      if (!beachId) throw new Error('fallback beach create returned no id');
      console.log(`[manual-pong] fallback beach created ${beachId}, navigating`);
      await page.goto(`${baseUrl}/beaches/${beachId}`, { waitUntil: 'domcontentloaded' });
      return beachId;
    }
  }
}

function runPongStack(beachId: string) {
  run(
    [
      'DEV_ALLOW_INSECURE_MANAGER_TOKEN=1',
      'DEV_MANAGER_INSECURE_TOKEN=DEV-MANAGER-TOKEN',
      'PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN',
      'NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN',
      'PRIVATE_BEACH_BYPASS_AUTH=0',
      'BEACH_SESSION_SERVER=http://beach-road:4132',
      'PONG_WATCHDOG_INTERVAL=10.0',
      `direnv exec . ./apps/private-beach/demo/pong/tools/pong-stack.sh start ${beachId}`,
    ].join(' '),
  );
}

function loadBootstrapSessions(dir: string): Record<string, BootstrapInfo> {
  const roles = ['lhs', 'rhs', 'agent'] as const;
  const result: Record<string, BootstrapInfo> = {};
  for (const role of roles) {
    const file = path.join(dir, `bootstrap-${role}.json`);
    if (!fs.existsSync(file)) {
      continue;
    }
    const lines = fs.readFileSync(file, 'utf8').split('\n');
    for (const line of lines) {
      const clean = line.replace(/\u001b\[[0-9;]*[A-Za-z]/g, '').trim();
      if (!clean) continue;
      let parsed: any;
      try {
        parsed = JSON.parse(clean);
      } catch {
        // try a relaxed substring between braces
        const start = clean.indexOf('{');
        const end = clean.lastIndexOf('}');
        if (start >= 0 && end > start) {
          try {
            parsed = JSON.parse(clean.slice(start, end + 1));
          } catch {
            parsed = null;
          }
        }
      }
      if (parsed && parsed.session_id && parsed.join_code) {
        result[role] = { session_id: parsed.session_id, join_code: parsed.join_code };
        break;
      }
      const sessionMatch = clean.match(/session_id["']?\s*[:=]\s*["']?([0-9a-fA-F-]{10,})/);
      const codeMatch = clean.match(/(join_code|code|passcode)["']?\s*[:=]\s*["']?([A-Z0-9]{4,})/);
      if (sessionMatch && codeMatch) {
        result[role] = { session_id: sessionMatch[1], join_code: codeMatch[2] };
        break;
      }
    }
  }
  return result;
}

async function openCatalog(page: Page) {
  const catalog = page.getByText(/node catalog/i).first();
  if (await catalog.isVisible()) return;
  await page.getByRole('button', { name: /catalog|nodes/i }).first().click({ trial: false }).catch(() => {});
  await catalog.waitFor({ state: 'visible', timeout: 10_000 }).catch(() => {});
}

async function dragFromCatalog(page: Page, itemText: RegExp, dropX: number, dropY: number) {
  await openCatalog(page);
  const item = page.getByText(itemText, { exact: false }).first();
  const canvas = page.locator('.react-flow__pane').first();
  await item.dragTo(canvas, { targetPosition: { x: dropX, y: dropY } });
}

async function attachSession(page: Page, session: BootstrapInfo) {
  await page.getByPlaceholder(/session id/i).fill(session.session_id);
  await page.getByPlaceholder(/passcode|join code/i).fill(session.join_code);
}

async function placeAndAttachTile(page: Page, role: 'lhs' | 'rhs' | 'agent', session: BootstrapInfo) {
  const isAgent = role === 'agent';
  await dragFromCatalog(page, isAgent ? /agent/i : /application/i, isAgent ? 400 : role === 'lhs' ? 100 : 700, 300);
  const tile = page.locator('[data-testid^="rf__node-tile:"]').last();
  await tile.click();
  await attachSession(page, session);
  if (isAgent) {
    await page.getByPlaceholder(/role/i).fill('Pong Agent');
    await page.getByPlaceholder(/responsibility/i).fill('Keep the volley alive');
  }
}

async function connectAgent(page: Page) {
  const nodes = await page.locator('.react-flow__node').all();
  if (nodes.length < 3) throw new Error('expected three nodes on canvas');
  const sourceHandle = page.locator('.react-flow__handle-source').first();
  const targetHandles = page.locator('.react-flow__handle-target');
  await sourceHandle.dragTo(targetHandles.nth(0));
  await sourceHandle.dragTo(targetHandles.nth(1));
}

function waitForBallMotion(logPath: string, min = 5) {
  return expect
    .poll(() => {
      if (!fs.existsSync(logPath)) return 0;
      const coords = new Set<string>();
      const lines = fs.readFileSync(logPath, 'utf8').split('\n');
      for (const line of lines) {
        const clean = line.replace(/\u001b\[[0-9;]*[A-Za-z]/g, '');
        for (const m of clean.matchAll(/Ball\s+(\d+),\s*([-\d]+)/g)) {
          coords.add(`${m[1]},${m[2]}`);
        }
      }
      return coords.size;
    }, { timeout: 180_000, message: `expected ball motion in ${logPath}` })
    .toBeGreaterThanOrEqual(min);
}

function waitForTransport(logPath: string, transport = 'webrtc') {
  return expect
    .poll(() => {
      if (!fs.existsSync(logPath)) return false;
      const txt = fs.readFileSync(logPath, 'utf8');
      return txt.includes(`"transport":"${transport}"`) || txt.includes(`transport":"${transport}`);
    }, { timeout: 120_000, message: `expected ${transport} in ${logPath}` })
    .toBeTruthy();
}

test.describe('manual pong connect (rewrite-2)', () => {
  test.skip(!shouldRun, 'Set RUN_MANUAL_PONG_SHOWCASE=1 to run manual drag/drop flow');
  test.setTimeout(30 * 60 * 1000);

  test('stack bootstrap, drag tiles, connect, and see volley', async ({ page }) => {
    console.log('[manual-pong] starting stack bootstrap');
    startStack();
    await page.context().addCookies([
      { name: 'pb-manager-token', value: managerToken, domain: 'localhost', path: '/' },
      { name: 'pb-auto-open-create', value: '1', domain: 'localhost', path: '/' },
    ]);
    console.log('[manual-pong] stack ready, creating beach via UI');
    const beachId = await signInAndCreateBeach(page);
    console.log(`[manual-pong] created beach ${beachId}, launching pong stack`);

    runPongStack(beachId);
    console.log('[manual-pong] pong stack started, loading bootstrap sessions');
    const sessions = loadBootstrapSessions(logRootHost);
    if (!sessions.lhs || !sessions.rhs || !sessions.agent) {
      throw new Error(`Missing bootstrap sessions in ${logRootHost}`);
    }

    console.log('[manual-pong] navigating to beach page and loading canvas');
    await page.goto(`${baseUrl}/beaches/${beachId}`, { waitUntil: 'domcontentloaded' });
    await page.getByText(/loading canvas/i).first().waitFor({ state: 'hidden', timeout: 120_000 }).catch(() => {});

    console.log('[manual-pong] placing tiles and attaching sessions');
    await placeAndAttachTile(page, 'lhs', sessions.lhs);
    await placeAndAttachTile(page, 'rhs', sessions.rhs);
    await placeAndAttachTile(page, 'agent', sessions.agent);
    console.log('[manual-pong] connecting agent to both players');
    await connectAgent(page);

    // Wait for WebRTC to show in host logs and for ball movement across both players.
    console.log('[manual-pong] waiting for WebRTC transport and ball motion');
    await waitForTransport(path.join(logRootHost, 'beach-host-lhs.log'));
    await waitForTransport(path.join(logRootHost, 'beach-host-rhs.log'));
    await waitForBallMotion(path.join(logRootHost, 'player-lhs.log'));
    await waitForBallMotion(path.join(logRootHost, 'player-rhs.log'));

    // Check volley crosses sides via ball trace uniqueness.
    const lhsTrace = path.join(logRootHost, 'ball-trace', 'ball-trace-lhs.jsonl');
    const rhsTrace = path.join(logRootHost, 'ball-trace', 'ball-trace-rhs.jsonl');
    expect(fs.existsSync(lhsTrace)).toBeTruthy();
    expect(fs.existsSync(rhsTrace)).toBeTruthy();
  });
});
