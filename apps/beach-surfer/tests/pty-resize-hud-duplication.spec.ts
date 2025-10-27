import { test, expect } from '@playwright/test';

/**
 * PTY Resize HUD Duplication Test
 *
 * This test reproduces the issue documented in:
 * docs/pty-resizing-issues/private-beach-duplicate-hud.md
 *
 * When a Private Beach tile is resized taller, newly exposed rows should
 * render as blank (MissingRow) until the PTY sends fresh content. This test
 * verifies that duplicate HUD content does not appear after resize.
 *
 * SETUP INSTRUCTIONS:
 * ===================
 * 1. Start a Beach session (e.g., Pong demo):
 *    ```
 *    cd apps/private-beach/demo/pong
 *    python3 tools/launch_session.py
 *    ```
 *
 * 2. Copy the session ID and passcode from the output
 *
 * 3. Update SESSION_ID and PASSCODE constants below
 *
 * 4. Ensure beach-road server is running on localhost:4132 (or update SESSION_SERVER)
 *
 * 5. Run the test:
 *    ```
 *    cd apps/beach-surfer
 *    npm run test:e2e:resize
 *    ```
 *
 * ENVIRONMENT VARIABLES (alternative to hardcoding):
 *   BEACH_TEST_SESSION_ID=<session-id>
 *   BEACH_TEST_PASSCODE=<passcode>
 *   BEACH_TEST_SESSION_SERVER=http://localhost:4132
 *   BEACH_TEST_SKIP_CONNECT=true  # Skip auto-connect, manual connection required
 */

// Configuration: Set via environment variables or hardcode for testing
const SESSION_ID = process.env.BEACH_TEST_SESSION_ID || 'REPLACE_WITH_YOUR_SESSION_ID';
const PASSCODE = process.env.BEACH_TEST_PASSCODE || 'REPLACE_WITH_YOUR_PASSCODE';
const BASE_URL = process.env.BEACH_TEST_URL || 'http://localhost:5173';
const SESSION_SERVER = process.env.BEACH_TEST_SESSION_SERVER || 'http://localhost:4132';
const SKIP_AUTO_CONNECT = process.env.BEACH_TEST_SKIP_CONNECT === 'true';

// Test timing configuration
const CONNECTION_TIMEOUT = 30000;
const RESIZE_SETTLE_TIME = 5000; // Time to wait after resize for backfill/replays (increased for large resizes)
const SCREENSHOT_DIR = 'test-results/resize';

test.describe('PTY Resize HUD Duplication', () => {
  test.beforeEach(async ({ page }) => {
    // Enable Beach trace logging before navigation
    await page.addInitScript(() => {
      (window as any).__BEACH_TRACE = true;
    });

    // Capture console logs for debugging
    page.on('console', (msg) => {
      const text = msg.text();
      console.log(`[BROWSER] ${text}`);
    });
  });

  test('should not duplicate HUD content when viewport expands', async ({ page }) => {
    // Validate configuration
    if (SESSION_ID === 'REPLACE_WITH_YOUR_SESSION_ID') {
      console.error('\n❌ ERROR: Please update SESSION_ID in the test file or set BEACH_TEST_SESSION_ID env var');
      console.error('See instructions at the top of tests/pty-resize-hud-duplication.spec.ts\n');
      test.skip();
      return;
    }

    // Navigate to beach-surfer
    if (SKIP_AUTO_CONNECT) {
      // Manual connection mode - just open the app
      console.log(`\n=== Manual connection mode ===`);
      console.log(`Please connect manually to session: ${SESSION_ID}`);
      await page.goto(BASE_URL);
    } else {
      // Auto-connect mode - try URL params with autoConnect, fallback to test API
      const url = `${BASE_URL}/?session=${SESSION_ID}&passcode=${PASSCODE}&sessionServer=${encodeURIComponent(SESSION_SERVER)}&autoConnect=true`;
      console.log(`\n=== Auto-connecting to session: ${SESSION_ID} via ${SESSION_SERVER} ===`);
      await page.goto(url);

      // Wait for page load and auto-connect to trigger
      await page.waitForTimeout(3000);

      // Check if terminal is visible (auto-connect worked)
      const terminalVisible = await page.locator('.beach-terminal').isVisible().catch(() => false);

      if (!terminalVisible) {
        // Auto-connect via URL failed, use programmatic test API
        console.log('=== Auto-connect via URL failed, using test API ===');
        const apiResult = await page.evaluate(
          ({ id, pass, server }) => {
            if ((window as any).__beachTestAPI) {
              (window as any).__beachTestAPI.connect(id, pass, server);
              return { success: true, method: 'testAPI' };
            }
            return { success: false, method: 'none' };
          },
          { id: SESSION_ID, pass: PASSCODE, server: SESSION_SERVER }
        );

        if (!apiResult.success) {
          throw new Error('Neither URL auto-connect nor test API available');
        }

        console.log(`=== Connected using ${apiResult.method} ===`);
      } else {
        console.log('=== Auto-connect via URL succeeded ===');
      }
    }

    // Wait for terminal to be visible and connected
    console.log('=== Waiting for terminal connection ===');
    await page.waitForSelector('.beach-terminal', { state: 'visible', timeout: CONNECTION_TIMEOUT });

    const statusBadge = page.locator('text=Connected');
    await expect(statusBadge).toBeVisible({ timeout: CONNECTION_TIMEOUT });

    // Give it extra time to fully sync
    console.log('=== Waiting for initial sync ===');
    await page.waitForTimeout(3000);

    // Capture initial state
    console.log('\n=== Capturing initial state ===');
    const initialState = await captureTerminalState(page, 'initial');
    await page.screenshot({
      path: `${SCREENSHOT_DIR}/01-before-resize.png`,
      fullPage: true
    });

    console.log(`Initial viewport rows: ${initialState.viewportRows}`);
    console.log(`Initial grid size: ${initialState.gridRows} rows x ${initialState.gridCols} cols`);
    console.log(`Initial visible rows: ${initialState.visibleRowCount}`);

    console.log('\n=== Initial Terminal Content ===');
    console.log(`Total rows: ${initialState.rows.length}`);
    // Show first 10 and last 10 rows for context
    const showFirst = Math.min(10, initialState.rows.length);
    const showLast = Math.min(10, initialState.rows.length);
    for (let i = 0; i < showFirst; i++) {
      const row = initialState.rows[i];
      const prefix = row.kind === 'missing' ? '[MISSING]' : '[LOADED] ';
      const truncatedText = row.text.length > 90 ? row.text.substring(0, 90) + '...' : row.text;
      console.log(`${String(i).padStart(3, ' ')}: ${prefix} "${truncatedText}"`);
    }
    if (initialState.rows.length > showFirst + showLast) {
      console.log(`... ${initialState.rows.length - showFirst - showLast} more rows ...`);
    }
    for (let i = Math.max(showFirst, initialState.rows.length - showLast); i < initialState.rows.length; i++) {
      const row = initialState.rows[i];
      const prefix = row.kind === 'missing' ? '[MISSING]' : '[LOADED] ';
      const truncatedText = row.text.length > 90 ? row.text.substring(0, 90) + '...' : row.text;
      console.log(`${String(i).padStart(3, ' ')}: ${prefix} "${truncatedText}"`);
    }

    // Simulate resize by dramatically increasing viewport to force many new rows
    // Target: ~70 rows × 90 cols to reproduce the HUD duplication issue
    // Line height is ~20px, so 70 rows = 1400px minimum
    const currentViewport = page.viewportSize();
    if (!currentViewport) {
      throw new Error('Could not get current viewport size');
    }

    const targetRows = 70;
    const targetCols = 90;
    const lineHeight = 20; // Typical line height
    const charWidth = 10; // Typical character width
    // Make the window extremely large to ensure the terminal container can expand
    // The beach-terminal component should fill available space
    const newHeight = targetRows * lineHeight + 200; // Add extra padding for UI chrome (status bar, etc)
    const newWidth = Math.max(targetCols * charWidth + 200, 1400); // Ensure wide enough + padding

    console.log(`\n=== Resizing viewport: ${currentViewport.width}x${currentViewport.height}px -> ${newWidth}x${newHeight}px ===`);
    console.log(`=== Target: ~${targetRows} rows × ${targetCols} cols ===`);

    await page.setViewportSize({
      width: newWidth,
      height: newHeight,
    });

    // Wait for resize to settle and any backfill/replay to occur
    console.log(`=== Waiting ${RESIZE_SETTLE_TIME}ms for resize to settle ===`);
    await page.waitForTimeout(RESIZE_SETTLE_TIME);

    // Capture state after resize
    console.log('\n=== Capturing state after resize ===');
    const afterResizeState = await captureTerminalState(page, 'after-resize');
    await page.screenshot({
      path: `${SCREENSHOT_DIR}/02-after-resize.png`,
      fullPage: true
    });

    console.log(`After-resize viewport rows: ${afterResizeState.viewportRows}`);
    console.log(`After-resize grid size: ${afterResizeState.gridRows} rows x ${afterResizeState.gridCols} cols`);
    console.log(`After-resize visible rows: ${afterResizeState.visibleRowCount}`);
    console.log(`Rows added: ${afterResizeState.visibleRowCount - initialState.visibleRowCount}`);

    console.log('\n=== After Resize Terminal Content ===');
    // Show first 20 and last 10 rows to capture any duplicate HUD at top
    const showFirstAfter = Math.min(20, afterResizeState.rows.length);
    const showLastAfter = Math.min(10, afterResizeState.rows.length);
    for (let i = 0; i < showFirstAfter; i++) {
      const row = afterResizeState.rows[i];
      const prefix = row.kind === 'missing' ? '[MISSING]' : '[LOADED] ';
      const truncatedText = row.text.length > 90 ? row.text.substring(0, 90) + '...' : row.text;
      console.log(`${String(i).padStart(3, ' ')}: ${prefix} "${truncatedText}"`);
    }
    if (afterResizeState.rows.length > showFirstAfter + showLastAfter) {
      console.log(`... ${afterResizeState.rows.length - showFirstAfter - showLastAfter} more rows ...`);
    }
    for (let i = Math.max(showFirstAfter, afterResizeState.rows.length - showLastAfter); i < afterResizeState.rows.length; i++) {
      const row = afterResizeState.rows[i];
      const prefix = row.kind === 'missing' ? '[MISSING]' : '[LOADED] ';
      const truncatedText = row.text.length > 90 ? row.text.substring(0, 90) + '...' : row.text;
      console.log(`${String(i).padStart(3, ' ')}: ${prefix} "${truncatedText}"`);
    }

    // Analyze the terminal grid for duplicate content
    console.log('\n=== Analyzing for duplicate HUD content ===');
    const analysis = analyzeGridForDuplicates(afterResizeState.rows);

    if (analysis.duplicates.length > 0) {
      console.error('\n❌ DUPLICATE HUD CONTENT DETECTED:');
      for (const dup of analysis.duplicates) {
        console.error(`  Row ${dup.firstIndex} and ${dup.secondIndex}: "${dup.text}"`);
      }
    } else {
      console.log('✓ No duplicate HUD content detected');
    }

    // Check that newly exposed rows are missing/blank
    console.log('\n=== Verifying newly exposed rows are blank ===');
    const newRowsCount = afterResizeState.visibleRowCount - initialState.visibleRowCount;
    console.log(`Expected ${newRowsCount} new rows to be visible`);

    if (newRowsCount > 0) {
      const newRows = afterResizeState.rows.slice(0, newRowsCount);
      const missingOrBlank = newRows.filter(row =>
        row.kind === 'missing' || row.text.trim() === ''
      );

      console.log(`New rows that are missing/blank: ${missingOrBlank.length}/${newRowsCount}`);

      // We expect most or all new rows to be blank until PTY sends fresh content
      // Allow some tolerance for edge cases, but the majority should be blank
      const blankRatio = missingOrBlank.length / newRowsCount;
      console.log(`Blank ratio: ${(blankRatio * 100).toFixed(1)}%`);

      if (blankRatio < 0.8) {
        console.warn(`⚠️  Warning: Only ${(blankRatio * 100).toFixed(1)}% of new rows are blank (expected >= 80%)`);
      }
    }

    // Main assertion: No duplicate HUD content
    expect(analysis.duplicates.length).toBe(0);

    console.log('\n=== Test completed successfully ===');
  });

  test('should capture detailed trace data for manual inspection', async ({ page }) => {
    // This test is for debugging - it captures extensive trace data
    // without strict assertions, useful for investigating issues

    // Validate configuration
    if (SESSION_ID === 'REPLACE_WITH_YOUR_SESSION_ID') {
      test.skip();
      return;
    }

    const url = `${BASE_URL}/?session=${SESSION_ID}&passcode=${PASSCODE}&sessionServer=${encodeURIComponent(SESSION_SERVER)}&autoConnect=true`;
    await page.goto(url);

    // Wait for auto-connect
    await page.waitForTimeout(3000);

    // Check if auto-connect worked, fallback to test API
    const terminalVisible = await page.locator('.beach-terminal').isVisible().catch(() => false);
    if (!terminalVisible) {
      await page.evaluate(
        ({ id, pass, server }) => {
          if ((window as any).__beachTestAPI) {
            (window as any).__beachTestAPI.connect(id, pass, server);
          }
        },
        { id: SESSION_ID, pass: PASSCODE, server: SESSION_SERVER }
      );
    }

    await page.waitForSelector('.beach-terminal', { state: 'visible', timeout: CONNECTION_TIMEOUT });
    await page.waitForTimeout(3000);

    // Capture before resize
    const beforeTraceData = await page.evaluate(() => {
      if (typeof (window as any).__BEACH_TRACE_DUMP_ROWS === 'function') {
        (window as any).__BEACH_TRACE_DUMP_ROWS();
        return (window as any).__BEACH_TRACE_LAST_ROWS;
      }
      return null;
    });

    console.log('\n=== Before Resize Trace Data ===');
    if (beforeTraceData) {
      console.log(JSON.stringify(beforeTraceData, null, 2));
    } else {
      console.log('No trace data available (__BEACH_TRACE_DUMP_ROWS not found)');
    }

    // Perform resize
    const currentViewport = page.viewportSize();
    if (currentViewport) {
      await page.setViewportSize({
        width: currentViewport.width,
        height: currentViewport.height + 300,
      });
      await page.waitForTimeout(RESIZE_SETTLE_TIME);
    }

    // Capture after resize
    const afterTraceData = await page.evaluate(() => {
      if (typeof (window as any).__BEACH_TRACE_DUMP_ROWS === 'function') {
        (window as any).__BEACH_TRACE_DUMP_ROWS();
        return (window as any).__BEACH_TRACE_LAST_ROWS;
      }
      return null;
    });

    console.log('\n=== After Resize Trace Data ===');
    if (afterTraceData) {
      console.log(JSON.stringify(afterTraceData, null, 2));
    }

    // Take final screenshot
    await page.screenshot({
      path: `${SCREENSHOT_DIR}/03-trace-debug.png`,
      fullPage: true
    });

    // This test always passes - it's just for data capture
    expect(true).toBe(true);
  });
});

// Helper types
interface TerminalRow {
  kind: 'loaded' | 'missing';
  absolute: number;
  text: string;
  seq?: number;
}

interface TerminalState {
  viewportRows: number;
  gridRows: number;
  gridCols: number;
  visibleRowCount: number;
  rows: TerminalRow[];
  baseRow: number;
  followTail: boolean;
}

interface DuplicatePattern {
  text: string;
  firstIndex: number;
  secondIndex: number;
}

interface DuplicateAnalysis {
  duplicates: DuplicatePattern[];
  suspiciousPatterns: string[];
}

/**
 * Capture the current terminal state including all visible rows
 */
async function captureTerminalState(page: any, label: string): Promise<TerminalState> {
  const state = await page.evaluate(() => {
    // Use the Beach trace API to dump rows
    if (typeof (window as any).__BEACH_TRACE_DUMP_ROWS === 'function') {
      (window as any).__BEACH_TRACE_DUMP_ROWS();
      const traceData = (window as any).__BEACH_TRACE_LAST_ROWS;

      if (traceData) {
        return {
          viewportRows: traceData.viewportHeight,
          gridRows: traceData.rowCount,
          gridCols: 80, // Beach uses 80 cols by default
          visibleRowCount: traceData.rows.length,
          baseRow: traceData.baseRow,
          followTail: traceData.followTail,
          rows: traceData.rows.map((row: any) => ({
            kind: row.kind,
            absolute: row.absolute,
            text: row.text || '',
            seq: row.seq,
          })),
        };
      }
    }
    return null;
  });

  if (!state) {
    // Fallback: extract text from DOM if trace API is not available
    const terminalText = await page.locator('.beach-terminal').textContent();
    const lines = terminalText?.split('\n') || [];

    return {
      viewportRows: lines.length,
      gridRows: lines.length,
      gridCols: 80, // default guess
      visibleRowCount: lines.length,
      baseRow: 0,
      followTail: true,
      rows: lines.map((text, index) => ({
        kind: 'loaded' as const,
        absolute: index,
        text,
      })),
    };
  }

  return state;
}

/**
 * Analyze terminal grid for duplicate HUD patterns
 *
 * Known HUD patterns from Pong demo:
 * - "Unknown command"
 * - "Commands"
 * - "Mode"
 * - ">"
 */
function analyzeGridForDuplicates(rows: TerminalRow[]): DuplicateAnalysis {
  const duplicates: DuplicatePattern[] = [];
  const suspiciousPatterns: string[] = [];

  // Common HUD patterns to watch for (from Pong demo)
  const hudPatterns = [
    'Unknown command',
    'Commands:',
    'Mode:',
    'Ready.',
  ];

  // Patterns to ignore (Pong game borders that naturally repeat)
  const ignorePatterns = [
    /^\|[\s|]*\|$/,  // Border rows like "|      |"
  ];

  // Check for consecutive identical non-blank rows
  for (let i = 0; i < rows.length - 1; i++) {
    const current = rows[i];
    const next = rows[i + 1];

    if (current.kind !== 'loaded' || next.kind !== 'loaded') {
      continue;
    }

    const currentText = current.text.trim();
    const nextText = next.text.trim();

    // Skip blank rows
    if (currentText === '' || nextText === '') {
      continue;
    }

    // Skip if both rows match an ignore pattern (e.g., Pong borders)
    const shouldIgnore = ignorePatterns.some(pattern =>
      pattern.test(currentText) && pattern.test(nextText)
    );
    if (shouldIgnore) {
      continue;
    }

    // Check for exact duplicates
    if (currentText === nextText) {
      duplicates.push({
        text: currentText,
        firstIndex: i,
        secondIndex: i + 1,
      });
    }

    // Check for HUD patterns appearing multiple times
    for (const pattern of hudPatterns) {
      if (currentText.includes(pattern)) {
        const count = rows.filter(row =>
          row.kind === 'loaded' && row.text.includes(pattern)
        ).length;

        if (count > 1 && !suspiciousPatterns.includes(pattern)) {
          suspiciousPatterns.push(pattern);
        }
      }
    }
  }

  return { duplicates, suspiciousPatterns };
}
