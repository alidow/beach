import { test, expect, Browser } from '@playwright/test';
import {
  captureTerminalState,
  analyzeGridForDuplicates,
  enableTraceLogging,
} from './helpers/terminal-capture';
import {
  findTileBySessionId,
  getTileSize,
  calculateTileDimensions,
  resizeTileByDragging,
} from './helpers/tile-manipulation';
import { signInWithClerk, getClerkToken } from './helpers/clerk-auth';
import {
  createBeach,
  attachSessionToBeach,
  deleteBeach,
  getSessionMetadata,
} from './helpers/beach-setup';

/**
 * Private Beach Tile Resize - HUD Duplication Test (Fully Automated)
 *
 * This test reproduces the issue documented in:
 * docs/pty-resizing-issues/private-beach-duplicate-hud.md
 *
 * When a Private Beach tile is resized taller (via react-grid-layout),
 * newly exposed terminal rows should render as blank (MissingRow) until
 * the PTY sends fresh content. This test verifies that duplicate HUD
 * content does not appear after resize.
 *
 * SETUP:
 * ======
 * Run the full test infrastructure with:
 *
 *   ./scripts/run-private-beach-e2e-tests.sh
 *
 * Or start infrastructure manually:
 *
 *   ./scripts/start-private-beach-tests.sh
 *   cd apps/private-beach && npm run test:e2e:tile-resize
 *   ./scripts/stop-private-beach-tests.sh
 */

// Test configuration
const RESIZE_SETTLE_TIME = 5000; // Wait after resize for PTY backfill/replay
const CONNECTION_TIMEOUT = 30000;
const SCREENSHOT_DIR = 'test-results/tile-resize';
// Note: If this username triggers OAuth (Google sign-in), you may need to:
// 1. Use a different test account configured for password auth in Clerk dashboard
// 2. Or provide the email address format instead of just username
const CLERK_USERNAME = process.env.CLERK_TEST_USERNAME || 'testuser';
const CLERK_PASSWORD = process.env.CLERK_TEST_PASSWORD || 'beach r0cks!';
const PRIVATE_BEACH_URL = 'http://localhost:3000';
const MANAGER_URL = process.env.BEACH_TEST_MANAGER_URL || 'http://localhost:8080';

test.describe('Private Beach Tile Resize - Automated', () => {
  const missingCredentials =
    !process.env.BEACH_TEST_SESSION_ID || !process.env.BEACH_TEST_PASSCODE;
  test.skip(missingCredentials, 'Session credentials not configured for tile resize automation.');

  let beachId: string;
  let authToken: string;
  let sessionId: string;
  let passcode: string;

  test.beforeAll(async ({ browser }: { browser: Browser }) => {
    console.log('\n=== Setting Up Test Beach ===');

    // Get session credentials from environment
    const sessionMetadata = getSessionMetadata();
    sessionId = sessionMetadata.sessionId;
    passcode = sessionMetadata.passcode;

    console.log(`Session ID: ${sessionId}`);
    console.log(`Passcode: ${passcode}`);

    // Create a separate page for setup
    const page = await browser.newPage();

    try {
      // Sign in with Clerk
      console.log('Signing in with Clerk...');
      await signInWithClerk(page, CLERK_USERNAME, CLERK_PASSWORD);

      // Get auth token
      console.log('Getting auth token...');
      authToken = await getClerkToken(page, 'private-beach-manager');
      console.log('Auth token acquired');

      // Create a new beach
      const beach = await createBeach(
        authToken,
        'E2E Test Beach - Tile Resize',
        `test-tile-resize-${Date.now()}`,
        MANAGER_URL
      );
      beachId = beach.id;

      console.log(`Beach created: ${beachId} (${beach.slug})`);

      // Attach the Pong session to the beach
      await attachSessionToBeach(authToken, beachId, sessionId, passcode, MANAGER_URL);

      console.log('Session attached to beach');
    } finally {
      await page.close();
    }
  });

  test.afterAll(async () => {
    console.log('\n=== Cleaning Up Test Beach ===');

    // Delete the beach
    if (beachId && authToken) {
      try {
        await deleteBeach(authToken, beachId, MANAGER_URL);
        console.log('Beach deleted successfully');
      } catch (error) {
        console.warn('Failed to delete beach:', error);
      }
    }
  });

  test.beforeEach(async ({ page }) => {
    // Enable Beach trace logging
    await enableTraceLogging(page);

    // Capture console logs for debugging
    page.on('console', (msg) => {
      const text = msg.text();
      if (text.includes('[beach-trace]') || text.includes('[tile-resize]')) {
        console.log(`[BROWSER] ${text}`);
      }
    });
  });

  test('should not duplicate HUD content when tile is resized significantly', async ({
    page,
  }) => {
    // Capture browser console logs for debugging
    page.on('console', (msg) => {
      const text = msg.text();
      // Filter for beach-related logs
      if (text.includes('[beach') || text.includes('transport') || text.includes('viewer') || text.includes('connect')) {
        console.log(`[BROWSER CONSOLE] ${text}`);
      }
    });

    console.log('\n=== Test Configuration ===');
    console.log(`Session ID: ${sessionId}`);
    console.log(`Beach ID: ${beachId}`);
    console.log(`Private Beach URL: ${PRIVATE_BEACH_URL}`);

    // Sign in with Clerk
    console.log('\n=== Signing In ===');
    await signInWithClerk(page, CLERK_USERNAME, CLERK_PASSWORD);

    // Navigate to the beach dashboard
    console.log('\n=== Navigating to Beach ===');
    await page.goto(`${PRIVATE_BEACH_URL}/beaches/${beachId}`);
    await page.waitForLoadState('domcontentloaded');
    console.log('Beach page loaded');

    // Open the explorer to add the session to the layout
    console.log('\n=== Opening Explorer ===');
    const explorerButton = page.getByRole('button', { name: 'Show Explorer' });
    await explorerButton.waitFor({ state: 'visible', timeout: 10000 });
    await explorerButton.click();
    console.log('Explorer opened');

    // Wait for session to appear in explorer (truncated ID)
    await page.waitForTimeout(2000);
    const truncatedId = sessionId.substring(0, 8);
    console.log(`Looking for session with truncated ID: ${truncatedId}`);

    // Take a screenshot to see the explorer state
    await page.screenshot({
      path: `${SCREENSHOT_DIR}/00-explorer-state.png`,
      fullPage: true,
    });

    const sessionInExplorer = page.getByText(truncatedId);
    await sessionInExplorer.waitFor({ state: 'visible', timeout: 10000 });
    console.log('Session found in explorer');

    // Add the session to the dashboard using the Add button
    console.log('\n=== Adding Session to Dashboard ===');

    // Close explorer first to access the Add button in the top nav
    const hideExplorerButton = page.getByRole('button', { name: 'Hide Explorer' });
    await hideExplorerButton.click();
    console.log('Closed explorer');
    await page.waitForTimeout(500);

    // Click the "Add" button in the top navigation
    const addButton = page.getByRole('button', { name: 'Add' });
    await addButton.click();
    console.log('Clicked Add button');
    await page.waitForTimeout(1000);

    // Click on "My Sessions" tab in the modal
    const mySessionsTab = page.getByRole('button', { name: 'My Sessions' });
    await mySessionsTab.click();
    console.log('Clicked My Sessions tab');
    await page.waitForTimeout(500);

    // Wait for the session to appear in the list
    const sessionItem = page.locator(`text=${truncatedId}`).locator('..').locator('..');
    await sessionItem.waitFor({ state: 'visible', timeout: 5000 });
    console.log('Session found in My Sessions list');

    // Click the Select checkbox for this session
    const selectCheckbox = sessionItem.getByRole('checkbox', { name: 'Select' });
    await selectCheckbox.click();
    console.log('Checked session checkbox');
    await page.waitForTimeout(500);

    // Click the "Attach N session(s)" button
    const attachButton = page.getByRole('button', { name: /Attach \d+ session/ });
    await attachButton.click();
    console.log('Clicked Attach button');

    // Wait for modal to close and layout to update
    await page.waitForTimeout(2000);

    // Reload the page to ensure viewer credentials are properly initialized
    console.log('Reloading page to initialize viewer credentials...');
    await page.reload({ waitUntil: 'networkidle' });
    // Wait longer for tokens to be fetched and terminal to initialize
    await page.waitForTimeout(5000);

    // Wait for the tile to appear on the page
    console.log('\n=== Waiting for Tile to Appear ===');
    const tileSelector = `[data-session-id="${sessionId}"]`;
    await page.waitForSelector(tileSelector, { timeout: CONNECTION_TIMEOUT });
    console.log('Tile appeared on page');

    const tile = page.locator(tileSelector);
    await expect(tile).toBeVisible();

    // Wait for terminal to connect - look for "healthy" badge which indicates connection
    console.log('\n=== Waiting for Terminal Connection ===');
    const healthyBadge = tile.locator('text=healthy');
    await healthyBadge.waitFor({ state: 'visible', timeout: CONNECTION_TIMEOUT });
    console.log('Terminal connected (healthy status visible)');

    // Check if Clerk session and token are available
    const clerkStatus = await page.evaluate(async () => {
      // @ts-ignore
      const clerk = window.Clerk;
      if (!clerk) return { hasClerk: false };

      const session = clerk.session;
      if (!session) return { hasClerk: true, hasSession: false };

      try {
        const token = await session.getToken({ template: 'private-beach-manager' });
        return { hasClerk: true, hasSession: true, hasToken: !!token, tokenLength: token?.length };
      } catch (e) {
        return { hasClerk: true, hasSession: true, tokenError: String(e) };
      }
    });
    console.log('Clerk status:', JSON.stringify(clerkStatus));

    // Check the actual text in the terminal area to see if it shows credential error
    const terminalText = await tile.locator('.relative.flex.min-h-0.flex-1').textContent();
    console.log('Terminal area text:', terminalText?.trim().substring(0, 100));

    // Check React component state - look for signs of viewerToken being null
    const hasXtermContainer = await page.locator(`${tileSelector} .xterm-helper-textarea`).count();
    console.log('Has xterm container:', hasXtermContainer > 0);

    // Wait for terminal content to appear (look for xterm rows)
    console.log('Waiting for terminal content to render...');
    await page.waitForSelector(`${tileSelector} .xterm-row`, { timeout: 15000 }).catch(() => {
      console.log('Warning: No terminal rows found, terminal may still be loading');
    });

    // Give terminal time to sync and receive content
    await page.waitForTimeout(5000);

    // Get initial tile size
    const initialSize = await getTileSize(page, sessionId);
    if (initialSize) {
      const dims = calculateTileDimensions(initialSize);
      console.log(`Initial tile: ${initialSize.w}w × ${initialSize.h}h (grid units)`);
      console.log(`Estimated: ~${dims.rows} rows × ${dims.cols} cols`);
    }

    // Capture initial state
    console.log('\n=== Capturing Initial Terminal State ===');
    const initialState = await captureTerminalState(page, 'initial');
    await page.screenshot({
      path: `${SCREENSHOT_DIR}/01-tile-before-resize.png`,
      fullPage: true,
    });

    console.log(`Viewport rows: ${initialState.viewportRows}`);
    console.log(`Grid rows: ${initialState.gridRows}`);
    console.log(`Visible rows: ${initialState.visibleRowCount}`);

    // Print sample of initial content
    console.log('\n=== Initial Content Sample (first 10, last 5 rows) ===');
    const showFirst = Math.min(10, initialState.rows.length);
    for (let i = 0; i < showFirst; i++) {
      const row = initialState.rows[i];
      const prefix = row.kind === 'missing' ? '[MISSING]' : '[LOADED] ';
      const text = row.text.length > 80 ? row.text.substring(0, 80) + '...' : row.text;
      console.log(`${String(i).padStart(3, ' ')}: ${prefix} "${text}"`);
    }
    if (initialState.rows.length > 15) {
      console.log('...');
      for (let i = initialState.rows.length - 5; i < initialState.rows.length; i++) {
        const row = initialState.rows[i];
        const prefix = row.kind === 'missing' ? '[MISSING]' : '[LOADED] ';
        const text = row.text.length > 80 ? row.text.substring(0, 80) + '...' : row.text;
        console.log(`${String(i).padStart(3, ' ')}: ${prefix} "${text}"`);
      }
    }

    // Resize the tile significantly (both width and height)
    console.log('\n=== Resizing Tile ===');
    console.log('Target: ~70 rows × 90 cols (significant resize)');

    // Calculate drag distance needed for height
    // Each grid row unit = 110px, we want ~70 terminal rows
    // 70 rows * 20px line height = 1400px terminal height
    // Plus ~100px chrome = 1500px total
    // 1500px / 110px per grid unit = ~14 grid units
    const targetGridH = 14;
    const currentGridH = initialSize?.h || 6;
    const deltaGridH = targetGridH - currentGridH;
    const deltaY = deltaGridH * 110; // 110px per grid row unit

    // Also resize width significantly
    const deltaX = 300; // Add significant width

    console.log(`Dragging resize handle +${deltaX}px horizontally, +${deltaY}px vertically`);

    await resizeTileByDragging(page, sessionId, deltaX, deltaY);

    // Wait for resize to settle and PTY to send backfill/replay
    console.log(`\n=== Waiting ${RESIZE_SETTLE_TIME}ms for PTY backfill ===`);
    await page.waitForTimeout(RESIZE_SETTLE_TIME);

    // Capture state after resize
    console.log('\n=== Capturing Terminal State After Resize ===');
    const afterResizeState = await captureTerminalState(page, 'after-resize');
    await page.screenshot({
      path: `${SCREENSHOT_DIR}/02-tile-after-resize.png`,
      fullPage: true,
    });

    console.log(`Viewport rows: ${afterResizeState.viewportRows}`);
    console.log(`Grid rows: ${afterResizeState.gridRows}`);
    console.log(`Visible rows: ${afterResizeState.visibleRowCount}`);
    console.log(
      `Rows added: ${afterResizeState.visibleRowCount - initialState.visibleRowCount}`
    );

    // Print sample of after-resize content (focus on top where duplicates would appear)
    console.log('\n=== After-Resize Content Sample (first 20, last 5 rows) ===');
    const showFirstAfter = Math.min(20, afterResizeState.rows.length);
    for (let i = 0; i < showFirstAfter; i++) {
      const row = afterResizeState.rows[i];
      const prefix = row.kind === 'missing' ? '[MISSING]' : '[LOADED] ';
      const text = row.text.length > 80 ? row.text.substring(0, 80) + '...' : row.text;
      console.log(`${String(i).padStart(3, ' ')}: ${prefix} "${text}"`);
    }
    if (afterResizeState.rows.length > 25) {
      console.log('...');
      for (let i = afterResizeState.rows.length - 5; i < afterResizeState.rows.length; i++) {
        const row = afterResizeState.rows[i];
        const prefix = row.kind === 'missing' ? '[MISSING]' : '[LOADED] ';
        const text = row.text.length > 80 ? row.text.substring(0, 80) + '...' : row.text;
        console.log(`${String(i).padStart(3, ' ')}: ${prefix} "${text}"`);
      }
    }

    // Verify content matches between initial and after resize
    console.log('\n=== Verifying Content Consistency ===');
    const commonRowCount = Math.min(
      initialState.visibleRowCount,
      afterResizeState.visibleRowCount
    );
    let mismatchCount = 0;

    for (let i = 0; i < commonRowCount; i++) {
      const initialRow = initialState.rows[i];
      const afterRow = afterResizeState.rows[i];

      if (
        initialRow.kind === 'loaded' &&
        afterRow.kind === 'loaded' &&
        initialRow.text !== afterRow.text
      ) {
        mismatchCount++;
        if (mismatchCount <= 5) {
          // Show first few mismatches
          console.log(`Mismatch at row ${i}:`);
          console.log(`  Before: "${initialRow.text}"`);
          console.log(`  After:  "${afterRow.text}"`);
        }
      }
    }

    if (mismatchCount > 0) {
      console.log(`⚠️  ${mismatchCount} rows changed after resize`);
    } else {
      console.log('✓ All common rows match');
    }

    // Analyze for duplicate HUD content
    console.log('\n=== Analyzing for Duplicate HUD Content ===');
    const analysis = analyzeGridForDuplicates(afterResizeState.rows);

    if (analysis.duplicates.length > 0) {
      console.error('\n❌ DUPLICATE HUD CONTENT DETECTED:');
      for (const dup of analysis.duplicates) {
        console.error(`  Row ${dup.firstIndex} and ${dup.secondIndex}: "${dup.text}"`);
      }
    } else {
      console.log('✓ No duplicate HUD content detected');
    }

    if (analysis.suspiciousPatterns.length > 0) {
      console.log('\n⚠️  Suspicious patterns found (appearing multiple times):');
      for (const pattern of analysis.suspiciousPatterns) {
        console.log(`  - "${pattern}"`);
      }
    }

    // Verify newly exposed rows
    console.log('\n=== Verifying Newly Exposed Rows ===');
    const newRowsCount = afterResizeState.visibleRowCount - initialState.visibleRowCount;
    console.log(`Expected ${newRowsCount} new rows to be visible`);

    if (newRowsCount > 0) {
      // Check that newly exposed rows at the top are missing/blank
      const newRows = afterResizeState.rows.slice(0, newRowsCount);
      const missingOrBlank = newRows.filter(
        (row) => row.kind === 'missing' || row.text.trim() === ''
      );

      console.log(`New rows that are missing/blank: ${missingOrBlank.length}/${newRowsCount}`);
      const blankRatio = missingOrBlank.length / newRowsCount;
      console.log(`Blank ratio: ${(blankRatio * 100).toFixed(1)}%`);

      // We expect most new rows to be blank (with tail padding)
      // Allow some tolerance for edge cases
      if (blankRatio < 0.7) {
        console.warn(
          `⚠️  Warning: Only ${(blankRatio * 100).toFixed(1)}% of new rows are blank (expected >= 70%)`
        );
      }
    }

    // Main assertion: No duplicate HUD content
    expect(analysis.duplicates.length).toBe(0);

    console.log('\n=== Test Completed Successfully ===');
  });
});
