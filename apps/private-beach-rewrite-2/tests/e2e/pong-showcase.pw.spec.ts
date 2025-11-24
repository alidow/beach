import { expect, Locator, test } from '@playwright/test';
import { clerk, clerkSetup } from '@clerk/testing/playwright';
import { execSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

type BootstrapInfo = { session_id: string; join_code: string };

const repoRoot = path.resolve(__dirname, '../../../..');
const logRoot = path.join(repoRoot, 'temp', 'pong-showcase');
const logDirInContainer = '/app/temp/pong-showcase';
const beachIdFile = path.join(logRoot, 'private-beach-id.txt');
const bootstrapDir = logRoot;
const managerToken =
  process.env.PRIVATE_BEACH_MANAGER_TOKEN || process.env.DEV_MANAGER_INSECURE_TOKEN || 'DEV-MANAGER-TOKEN';
const clerkUser = process.env.CLERK_USER || 'test@beach.sh';
const clerkPass = process.env.CLERK_PASS || 'h3llo Beach';
const baseUrl = (process.env.PRIVATE_BEACH_REWRITE_URL || 'http://localhost:3003').replace(/\/$/, '');
const shouldRun = process.env.RUN_PONG_SHOWCASE === '1';
const hasClerkSecrets = Boolean(process.env.CLERK_SECRET_KEY) && Boolean(process.env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY);

const execEnv = {
  ...process.env,
  PRIVATE_BEACH_MANAGER_URL: 'http://localhost:8080',
  PONG_SESSION_SERVER: process.env.PONG_SESSION_SERVER || 'http://beach-road:4132/',
  PONG_AUTH_GATEWAY: process.env.PONG_AUTH_GATEWAY || 'http://beach-gate:4133',
  PONG_STACK_MANAGER_HEALTH_ATTEMPTS: '120',
  PONG_LOG_ROOT: logDirInContainer,
  PONG_LOG_DIR: logDirInContainer,
  PONG_FRAME_DUMP_DIR: path.join(logDirInContainer, 'frame-dumps'),
  PONG_BALL_TRACE_DIR: path.join(logDirInContainer, 'ball-trace'),
  PONG_COMMAND_TRACE_DIR: path.join(logDirInContainer, 'command-trace'),
  PONG_CREATED_BEACH_ID_FILE: beachIdFile,
  BEACH_FRAMED_CHUNK_SIZE: '1048576',
  PONG_DISABLE_HOST_TOKEN: '1',
  HOST_MANAGER_TOKEN: managerToken,
  PRIVATE_BEACH_MANAGER_TOKEN: managerToken,
  NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_TOKEN: managerToken,
};

function run(cmd: string) {
  execSync(cmd, {
    cwd: repoRoot,
    stdio: 'inherit',
    env: execEnv,
    timeout: 5 * 60 * 1000,
  });
}

function loadBootstrapSessions(dir: string): BootstrapInfo[] {
  const files = ['bootstrap-lhs.json', 'bootstrap-rhs.json', 'bootstrap-agent.json'];
  const sessions: BootstrapInfo[] = [];
  for (const file of files) {
    const full = path.join(dir, file);
    if (!fs.existsSync(full)) continue;
    try {
      const raw = fs.readFileSync(full, 'utf8');
      const firstBrace = raw.indexOf('{');
      const lastBrace = raw.lastIndexOf('}');
      if (firstBrace < 0 || lastBrace <= firstBrace) continue;
      const parsed = JSON.parse(raw.slice(firstBrace, lastBrace + 1));
      if (parsed.session_id && parsed.join_code) {
        sessions.push({ session_id: parsed.session_id, join_code: parsed.join_code });
      }
    } catch {
      continue;
    }
  }
  return sessions;
}

async function assertTileMovement(tileBody: Locator) {
  const normalize = (text: string) => text.replace(/\s+/g, ' ').trim();
  const initial = normalize(await tileBody.innerText());
  await expect
    .poll(async () => {
      const next = normalize(await tileBody.innerText());
      return next !== initial ? next : '';
    }, { timeout: 45_000, message: 'expected tile content to change (paddle/ball motion)' })
    .not.toBe('');
}

async function waitForBallMotion(logPath: string, minPositions = 3, timeoutMs = 180_000) {
  await expect
    .poll(() => {
      const tracePath = logPath.replace(/player-(lhs|rhs)\.log$/, 'ball-trace/ball-trace-$1.jsonl');
      if (fs.existsSync(tracePath)) {
        const coords = new Set<string>();
        const contents = fs.readFileSync(tracePath, 'utf8');
        for (const line of contents.split('\n')) {
          if (!line.trim()) continue;
          try {
            const entry = JSON.parse(line);
            if (typeof entry.x === 'number' && typeof entry.y === 'number') {
              // Normalize to integer buckets to avoid noise.
              coords.add(`${Math.round(entry.x)},${Math.round(entry.y)}`);
            }
          } catch {
            continue;
          }
        }
        return coords.size;
      }
      if (!fs.existsSync(logPath)) return 0;
      const lines = fs.readFileSync(logPath, 'utf8').split('\n');
      const coords = new Set<string>();
      for (const line of lines) {
        const clean = line.replace(/\u001b\[[0-9;]*[A-Za-z]/g, '');
        const matches = clean.matchAll(/Ball\s+(\d+),\s*([-\d]+)/g);
        for (const match of matches) {
          coords.add(`${match[1]},${match[2]}`);
        }
      }
      return coords.size;
    }, { timeout: timeoutMs, message: `expected ball positions to change in ${logPath}` })
    .toBeGreaterThanOrEqual(minPositions);
  // eslint-disable-next-line no-console
  console.log(`[pong-test] ball motion observed in ${logPath}`);
}

async function waitForPaddleMotion(logPath: string, minDistinct = 2, timeoutMs = 120_000) {
  await expect
    .poll(() => {
      if (!fs.existsSync(logPath)) return 0;
      const lines = fs.readFileSync(logPath, 'utf8').split('\n');
      const paddles = new Set<string>();
      for (const line of lines) {
        const clean = line.replace(/\u001b\[[0-9;]*[A-Za-z]/g, '');
        const matches = clean.matchAll(/paddle=([-\d.]+)/g);
        for (const match of matches) {
          // ignore placeholder dash values
          if (match[1] === '–' || match[1] === '-') continue;
          paddles.add(match[1]);
        }
      }
      return paddles.size;
    }, { timeout: timeoutMs, message: `expected paddle positions to change in ${logPath}` })
    .toBeGreaterThanOrEqual(minDistinct);
  // eslint-disable-next-line no-console
  console.log(`[pong-test] paddle motion observed in ${logPath}`);
}

test.describe('pong showcase fast-path (rewrite-2)', () => {
  test.skip(!shouldRun, 'Set RUN_PONG_SHOWCASE=1 to run the full dockerized pong showcase');
  test.skip(!hasClerkSecrets, 'Clerk dev keys required for programmatic sign-in');
  test.setTimeout(20 * 60 * 1000);

  test.beforeAll(async () => {
    fs.rmSync(logRoot, { recursive: true, force: true });
    fs.mkdirSync(logRoot, { recursive: true });

    run('direnv allow');
    run('direnv exec . ./scripts/dockerdown --postgres-only');
    run('direnv exec . docker compose down');
    run("direnv exec . env BEACH_SESSION_SERVER='http://beach-road:4132' PONG_WATCHDOG_INTERVAL=10.0 docker compose build beach-manager");
    run(
      [
        'DEV_ALLOW_INSECURE_MANAGER_TOKEN=1',
        'DEV_MANAGER_INSECURE_TOKEN=DEV-MANAGER-TOKEN',
        'PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN',
        'PRIVATE_BEACH_BYPASS_AUTH=0',
        'direnv exec . sh -c \'BEACH_SESSION_SERVER="http://beach-road:4132" PONG_WATCHDOG_INTERVAL=10.0 BEACH_MANAGER_STDOUT_LOG=trace BEACH_MANAGER_FILE_LOG=trace BEACH_MANAGER_TRACE_DEPS=1 docker compose up -d\'',
      ].join(' '),
    );
    run('direnv exec . apps/private-beach/demo/pong/tools/pong-stack.sh --setup-beach start -- create-beach');

    if (!fs.existsSync(beachIdFile)) {
      throw new Error(`Beach id file missing at ${beachIdFile}`);
    }
  });

  test('tiles connect and gameplay is visible', async ({ page }) => {
    const beachId = fs.readFileSync(beachIdFile, 'utf8').trim();
    if (!beachId) {
      throw new Error('Empty private beach id; did pong-stack create it?');
    }
    // eslint-disable-next-line no-console
    console.log(`[pong-test] running showcase for beach ${beachId}`);

    const bootstrapSessions = loadBootstrapSessions(bootstrapDir);
    // eslint-disable-next-line no-console
    console.log('[pong-test] bootstrap sessions', bootstrapSessions);

    await clerkSetup({
      secretKey: process.env.CLERK_SECRET_KEY,
      publishableKey: process.env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY,
    });

    page.on('console', (msg) => {
      if (msg.type() === 'error') {
        // eslint-disable-next-line no-console
        console.error('[browser]', msg.text());
      }
    });

    await page.addInitScript(() => {
      const anyWindow = window as unknown as Record<string, unknown>;
      const events: Array<{ event: string; payload: any }> = [];
      anyWindow.__telemetry_log__ = events;
      anyWindow.__BEACH_TELEMETRY__ = (event: string, payload: any) => {
        events.push({ event, payload });
      };
    });

    await page.goto(baseUrl, { waitUntil: 'domcontentloaded' });
    await clerk.signIn({
      page,
      signInParams: { strategy: 'password', identifier: clerkUser, password: clerkPass },
    });

    const beachUrl = `${baseUrl}/beaches/${beachId}`;
    await page.goto(beachUrl, { waitUntil: 'domcontentloaded' });

    const canvasShell = page.locator('[data-private-beach-rewrite]');
    await canvasShell.waitFor({ state: 'visible', timeout: 120_000 });
    await page.getByText(/loading canvas/i).first().waitFor({ state: 'hidden', timeout: 120_000 }).catch(() => {});
    // eslint-disable-next-line no-console
    console.log('[pong-test] canvas loaded');

    const tiles = page.locator('[data-testid^="rf__node-tile:"]');
    await expect
      .poll(async () => tiles.count(), {
        timeout: 120_000,
        message: 'expected pong tiles to render',
      })
      .toBeGreaterThanOrEqual(bootstrapSessions.length || 3);
    // eslint-disable-next-line no-console
    console.log('[pong-test] tiles rendered');

    const requiredConnected = ['Pong LHS', 'Pong RHS'];
    for (const name of requiredConnected) {
      const tile = tiles.filter({ hasText: name }).first();
      await expect(tile.getByText(/connected/i)).toBeVisible({ timeout: 120_000 });
    }
    const agentTile = tiles.filter({ hasText: 'Pong Agent' }).first();
    await expect(agentTile).toBeVisible({ timeout: 120_000 });
    // eslint-disable-next-line no-console
    console.log('[pong-test] tiles connected');

    const lhsLog = path.join(logRoot, 'player-lhs.log');
    const rhsLog = path.join(logRoot, 'player-rhs.log');
    const agentLog = path.join(logRoot, 'agent.log');
    // eslint-disable-next-line no-console
    console.log('[pong-test] checking ball motion via logs', { lhsLog, rhsLog });
    await page.waitForTimeout(5_000);
    await waitForBallMotion(lhsLog);
    await waitForBallMotion(rhsLog, 2);
    await waitForPaddleMotion(agentLog);

    const telemetry = await page.evaluate(() => {
      const anyWindow = window as unknown as Record<string, any>;
      return (anyWindow.__telemetry_log__ as Array<{ event: string; payload: any }>) ?? [];
    });
    const failureEvents = telemetry.filter((e) => /failure|error/i.test(e.event));
    expect(failureEvents, 'telemetry failures should be empty').toHaveLength(0);
  });
});
