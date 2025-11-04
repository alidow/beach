import { expect, test } from '@playwright/test';

function buildSandboxUrl(): string {
  const params = new URLSearchParams({
    skipApi: '1',
    privateBeachId: 'sandbox',
    sessions: 'sandbox-session|application|Sandbox Fixture',
    terminalFixtures: 'sandbox-session:pong-lhs',
    viewerToken: 'sandbox-token',
    tileWidth: '448',
    tileHeight: '448',
    rewrite: '1',
  });
  return `/dev/private-beach-sandbox?${params.toString()}`;
}

test('rewrite telemetry emits expected canvas events', async ({ page }) => {
  await page.addInitScript(() => {
    const anyWindow = window as unknown as Record<string, unknown>;
    const events: Array<{ event: string; payload: any }> = [];
    anyWindow.__telemetry_log__ = events;
    anyWindow.__BEACH_TELEMETRY__ = (event: string, payload: any) => {
      events.push({ event, payload });
    };
  });

  await page.goto(buildSandboxUrl());

  const tile = page.getByTestId('rf__node-tile:sandbox-session');
  await expect(tile).toBeVisible();

  await page.waitForFunction(() => {
    const anyWindow = window as unknown as Record<string, unknown>;
    const events = anyWindow.__telemetry_log__ as Array<{ event: string; payload: any }> | undefined;
    if (!events) return false;
    return events.some((entry) => entry.event === 'canvas.tile.connect.success');
  });

  const events = await page.evaluate<Array<{ event: string; payload: any }>>(() => {
    const anyWindow = window as unknown as Record<string, unknown>;
    return (anyWindow.__telemetry_log__ as Array<{ event: string; payload: any }>) ?? [];
  });

  const eventNames = events.map((entry) => entry.event);
  expect(eventNames).toEqual(
    expect.arrayContaining([
      'canvas.rewrite.flag-state',
      'canvas.tile.create',
      'canvas.tile.connect.start',
      'canvas.tile.connect.success',
    ]),
  );

  const flagEvents = events.filter((entry) => entry.event === 'canvas.rewrite.flag-state');
  expect(flagEvents.some((entry) => (entry.payload?.enabled as boolean | undefined) === true)).toBe(true);

  const createEvent = events.find((entry) => entry.event === 'canvas.tile.create');
  expect(createEvent?.payload?.sessionId).toBe('sandbox-session');

  const successEvent = events.find((entry) => entry.event === 'canvas.tile.connect.success');
  expect(successEvent?.payload?.sessionId).toBe('sandbox-session');
  expect(successEvent?.payload?.latencyMs).not.toBeUndefined();
});
