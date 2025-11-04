import { expect, test } from '@playwright/test';

const SESSION = {
  id: '6b563c2d-f628-4e77-b992-a121317aed9e',
  joinCode: 'ORXVCU',
  managerUrl: 'http://localhost:4132/',
  title: 'Pong LHS (live)',
};

function buildSandboxUrl(): string {
  const params = new URLSearchParams({
    skipApi: '1',
    privateBeachId: 'sandbox',
    managerUrl: SESSION.managerUrl,
    sessions: `${SESSION.id}|application|${SESSION.title}`,
    passcodes: `${SESSION.id}:${SESSION.joinCode}`,
    titles: `${SESSION.id}:${SESSION.title}`,
    terminalFixtures: `${SESSION.id}:pong-lhs`,
    rewrite: '1',
  });
  return `/dev/private-beach-sandbox?${params.toString()}`;
}

test.describe.configure({ mode: 'serial' });

test('adds live session tile to sandbox canvas', async ({ page }) => {
  test.setTimeout(90_000);
  await page.goto(buildSandboxUrl(), { waitUntil: 'networkidle' });

  const tile = page.getByTestId(`rf__node-tile:${SESSION.id}`);
  await expect(tile).toBeVisible({ timeout: 30_000 });
  await expect(tile.getByRole('button', { name: SESSION.title })).toBeVisible({ timeout: 30_000 });

  const placeholder = page.getByText('Preparing terminal preview…');
  await expect(placeholder).toHaveCount(0, { timeout: 60_000 });

  await expect(page.locator('body')).toContainText('PRIVATE BEACH PONG', { timeout: 60_000 });
});
