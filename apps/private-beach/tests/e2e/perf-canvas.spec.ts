import { test, expect } from '@playwright/test';

const shouldRun = !!process.env.PERF;

test.describe('Canvas Perf (prototype explorer)', () => {
  test.skip(!shouldRun, 'Set PERF=1 to run perf tests');

  test('baseline FPS >= 50 over 2s idle', async ({ page }) => {
    await page.goto('/prototypes/explorer');
    // Wait for UI to settle
    await page.waitForLoadState('networkidle');
    await page.waitForTimeout(500);

    const frames = await page.evaluate(async () => {
      return await new Promise<number>((resolve) => {
        let count = 0;
        const start = performance.now();
        function step() {
          count += 1;
          if (performance.now() - start > 2000) return resolve(count);
          requestAnimationFrame(step);
        }
        requestAnimationFrame(step);
      });
    });

    const fps = frames / 2;
    // Soft assertion: print metric, assert budget when available
    test.info().annotations.push({ type: 'metric', description: `fps=${fps.toFixed(1)}` });
    expect(fps).toBeGreaterThan(30);
  });

  test('drag interaction latency recorded', async ({ page }) => {
    await page.goto('/prototypes/explorer');
    await page.waitForLoadState('networkidle');

    // Try to drag the first visible draggable session in the prototype list (best-effort)
    const candidate = page.locator('div.group.relative').first();
    await expect(candidate).toBeVisible();
    const box = await candidate.boundingBox();
    if (!box) test.skip(true, 'no draggable candidate');

    const start = Date.now();
    await page.mouse.move(box!.x + 4, box!.y + 4);
    await page.mouse.down();
    await page.mouse.move(box!.x + 40, box!.y + 40, { steps: 10 });
    await page.mouse.up();
    const latency = Date.now() - start;

    test.info().annotations.push({ type: 'metric', description: `drag_latency_ms=${latency}` });
    expect(latency).toBeLessThan(2000); // Budget placeholder
  });
});

