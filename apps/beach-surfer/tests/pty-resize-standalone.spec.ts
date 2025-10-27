/**
 * Standalone PTY resize test against beach-surfer (no Private Beach dependencies)
 * Tests resize behavior with a live Beach session
 */

import { test, expect } from '@playwright/test';

const SESSION_ID = process.env.BEACH_TEST_SESSION_ID || '';
const PASSCODE = process.env.BEACH_TEST_PASSCODE || '';
const SESSION_SERVER = process.env.BEACH_TEST_SESSION_SERVER || 'http://localhost:8080';

test.describe('PTY Resize - Standalone', () => {
  test.skip(!SESSION_ID || !PASSCODE, 'BEACH_TEST_SESSION_ID and BEACH_TEST_PASSCODE required');

  test('should not duplicate HUD content when resized to 70+ rows', async ({ page }) => {
    // Navigate to beach-surfer
    await page.goto('http://localhost:5173');

    // Enable trace logging
    await page.evaluate(() => {
      (window as any).__BEACH_TRACE = true;
    });

    // Fill in session credentials (use placeholder text to identify inputs)
    const sessionIdInput = page.locator('input').first(); // SESSION ID input
    const passcodeInput = page.locator('input').nth(1); // PASSCODE input

    await sessionIdInput.fill(SESSION_ID);
    await passcodeInput.fill(PASSCODE);

    // Expand advanced settings to set session server
    await page.click('text=Advanced settings');
    await page.waitForTimeout(500);

    const sessionServerInput = page.locator('input').nth(2); // Session server input
    await sessionServerInput.clear();
    await sessionServerInput.fill(SESSION_SERVER);

    // Connect
    await page.click('button:has-text("Connect")');

    // Wait for connection
    await page.waitForTimeout(3000);

    // Capture initial state
    const initialState = await page.evaluate(() => {
      if (typeof (window as any).__BEACH_TRACE_DUMP_ROWS === 'function') {
        (window as any).__BEACH_TRACE_DUMP_ROWS();
        return (window as any).__BEACH_TRACE_LAST_ROWS;
      }
      return null;
    });

    console.log('Initial state:', {
      viewportRows: initialState?.viewportHeight,
      gridRows: initialState?.rowCount,
      rowsLoaded: initialState?.rows?.length,
    });

    // Resize browser window to achieve 70+ rows
    await page.setViewportSize({ width: 1400, height: 1800 });
    await page.waitForTimeout(5000); // Wait for PTY resize and backfill

    // Capture after resize
    const afterResizeState = await page.evaluate(() => {
      if (typeof (window as any).__BEACH_TRACE_DUMP_ROWS === 'function') {
        (window as any).__BEACH_TRACE_DUMP_ROWS();
        return (window as any).__BEACH_TRACE_LAST_ROWS;
      }
      return null;
    });

    console.log('After resize:', {
      viewportRows: afterResizeState?.viewportHeight,
      gridRows: afterResizeState?.rowCount,
      rowsLoaded: afterResizeState?.rows?.length,
    });

    // Analyze for duplicates
    const rows = afterResizeState?.rows || [];
    const ignorePatterns = [/^\|[\s|]*\|$/]; // Ignore Pong borders

    const textRows = rows
      .map((r: any) => r.text || '')
      .filter((text: string) => !ignorePatterns.some(p => p.test(text)));

    const duplicates: Array<{ row1: number; row2: number; text: string }> = [];
    for (let i = 0; i < textRows.length - 1; i++) {
      for (let j = i + 1; j < textRows.length; j++) {
        if (textRows[i].trim() && textRows[i] === textRows[j]) {
          duplicates.push({ row1: i, row2: j, text: textRows[i] });
        }
      }
    }

    console.log(`Found ${duplicates.length} duplicate rows`);
    if (duplicates.length > 0) {
      console.log('Duplicates:', duplicates.slice(0, 5));
    }

    expect(duplicates.length).toBe(0);
  });
});
