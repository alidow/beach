/*
  Simple perf harness: launches chromium, navigates to the prototype explorer,
  measures a short rAF FPS sample and emits a JSON report to test-results/.

  Usage:
    pnpm --filter @beach/private-beach exec tsx tests/scripts/canvas-perf.ts
*/
import fs from 'node:fs';
import path from 'node:path';
import { chromium } from 'playwright';

async function measureFps(pageUrl: string) {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  try {
    await page.goto(pageUrl, { waitUntil: 'domcontentloaded' });
    await page.waitForLoadState('networkidle');
    await page.waitForTimeout(500);
    const frames: number = await page.evaluate(async () => {
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
    return { fps };
  } finally {
    await page.close();
    await browser.close();
  }
}

async function main() {
  const baseUrl = process.env.BASE_URL || 'http://localhost:3000';
  const url = `${baseUrl.replace(/\/$/, '')}/prototypes/explorer`;
  const result = await measureFps(url);
  const outDir = path.resolve(process.cwd(), 'test-results');
  fs.mkdirSync(outDir, { recursive: true });
  const outPath = path.join(outDir, 'canvas-perf.json');
  fs.writeFileSync(outPath, JSON.stringify({ url, ...result, at: Date.now() }, null, 2));
  // eslint-disable-next-line no-console
  console.log(`Wrote perf report to ${outPath}`);
}

main().catch((err) => {
  // eslint-disable-next-line no-console
  console.error(err);
  process.exit(1);
});

