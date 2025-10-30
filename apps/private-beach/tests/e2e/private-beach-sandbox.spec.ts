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
