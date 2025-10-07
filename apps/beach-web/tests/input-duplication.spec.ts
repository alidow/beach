import { test, expect } from '@playwright/test';

test.describe('Input Duplication Bug', () => {
  test('should not duplicate input characters', async ({ page }) => {
    // Enable trace logging
    await page.addInitScript(() => {
      (window as any).__BEACH_TRACE = true;
    });

    // Listen to console messages to capture trace logs
    const consoleLogs: string[] = [];
    page.on('console', msg => {
      const text = msg.text();
      consoleLogs.push(text);
      console.log(`[BROWSER] ${text}`);
    });

    // Navigate to the join page with the session ID and passcode
    const sessionId = '449b4de0-9697-4b7b-b9cb-d044c6b5a248';
    const passcode = '933254';
    await page.goto(`http://localhost:5173/?session=${sessionId}&passcode=${passcode}&autoConnect=true`);

    // Wait for connection
    await page.waitForTimeout(3000);

    // Wait for the terminal to be connected
    await page.waitForSelector('.beach-terminal', { state: 'visible' });

    // Look for connection status
    const statusBadge = page.locator('text=Connected').or(page.locator('text=Connecting'));
    await expect(statusBadge).toBeVisible({ timeout: 10000 });

    // Wait a bit more for the connection to fully establish
    await page.waitForTimeout(2000);

    // Focus on the terminal
    const terminal = page.locator('.beach-terminal');
    await terminal.click();
    await page.waitForTimeout(500);

    // Type "echo hello" character by character to test for duplications
    const testInputs = [
      { key: 'e', expected: 'e' },
      { key: 'c', expected: 'c' },
      { key: 'h', expected: 'h' },
      { key: 'o', expected: 'o' },
      { key: ' ', expected: ' ' },
      { key: 'h', expected: 'h' },
      { key: 'e', expected: 'e' },
      { key: 'l', expected: 'l' },
      { key: 'l', expected: 'l' },
      { key: 'o', expected: 'o' },
    ];

    for (const input of testInputs) {
      console.log(`\n=== Typing key: "${input.key}" ===`);
      await page.keyboard.press(input.key);
      await page.waitForTimeout(100);
    }

    // Press Enter
    console.log('\n=== Typing Enter ===');
    await page.keyboard.press('Enter');
    await page.waitForTimeout(1000);

    // Get terminal content
    const terminalText = await terminal.textContent();
    console.log('\n=== Terminal Content ===');
    console.log(terminalText);

    // Take a screenshot
    await page.screenshot({ path: 'test-results/input-duplication.png', fullPage: true });

    // Analyze console logs for patterns
    console.log('\n=== Analyzing Console Logs ===');
    const keydownLogs = consoleLogs.filter(log => log.includes('handleKeyDown: sending input'));
    console.log(`Total handleKeyDown events: ${keydownLogs.length}`);

    const encodeKeyLogs = consoleLogs.filter(log => log.includes('encodeKeyEvent:'));
    console.log(`Total encodeKeyEvent calls: ${encodeKeyLogs.length}`);

    // Check for duplicate sends for the same key
    const keyPressCounts = new Map<string, number>();
    for (const log of keydownLogs) {
      const match = log.match(/"key":"(\w| )"/);
      if (match) {
        const key = match[1];
        keyPressCounts.set(key, (keyPressCounts.get(key) || 0) + 1);
      }
    }

    console.log('\n=== Key Press Counts ===');
    for (const [key, count] of keyPressCounts.entries()) {
      console.log(`Key "${key}": ${count} times`);
    }

    // Check if there are duplicates in the terminal output
    // The pattern from the bug report shows characters appearing twice
    const bugPattern1 = /eecho/; // "echo" typed once but appears as "eecho"
    const bugPattern2 = /aasdf/; // Multiple consecutive duplications

    const hasDuplication = bugPattern1.test(terminalText || '') || bugPattern2.test(terminalText || '');

    if (hasDuplication) {
      console.error('\n❌ DUPLICATION DETECTED in terminal output!');
      console.error('Terminal text:', terminalText);
    } else {
      console.log('\n✓ No obvious duplication pattern detected');
    }

    // The test should fail if we detect input duplication
    expect(hasDuplication).toBe(false);
  });

  test('should trace all keyboard events', async ({ page }) => {
    // Enable trace logging
    await page.addInitScript(() => {
      (window as any).__BEACH_TRACE = true;
    });

    // Capture all keyboard events at the browser level
    await page.evaluateOnNewDocument(() => {
      const events: any[] = [];
      (window as any).__keyboardEvents = events;

      ['keydown', 'keyup', 'keypress'].forEach(eventType => {
        document.addEventListener(eventType, (e: Event) => {
          const ke = e as KeyboardEvent;
          events.push({
            type: eventType,
            key: ke.key,
            code: ke.code,
            timestamp: Date.now(),
            repeat: ke.repeat,
          });
        }, true);
      });
    });

    // Navigate to the join page
    const sessionId = '449b4de0-9697-4b7b-b9cb-d044c6b5a248';
    const passcode = '933254';
    await page.goto(`http://localhost:5173/?session=${sessionId}&passcode=${passcode}&autoConnect=true`);

    await page.waitForTimeout(3000);
    await page.waitForSelector('.beach-terminal', { state: 'visible' });

    const terminal = page.locator('.beach-terminal');
    await terminal.click();
    await page.waitForTimeout(500);

    // Type a test string
    await page.keyboard.type('test', { delay: 100 });
    await page.waitForTimeout(500);

    // Get the keyboard events
    const events = await page.evaluate(() => {
      return (window as any).__keyboardEvents;
    });

    console.log('\n=== Keyboard Events ===');
    console.log(JSON.stringify(events, null, 2));

    // Check for suspicious patterns
    const keydownEvents = events.filter((e: any) => e.type === 'keydown');
    const repeatedEvents = keydownEvents.filter((e: any) => e.repeat === true);

    console.log(`\nTotal keydown events: ${keydownEvents.length}`);
    console.log(`Repeated events: ${repeatedEvents.length}`);

    if (repeatedEvents.length > 0) {
      console.log('\n⚠️  Found repeated keyboard events:');
      console.log(JSON.stringify(repeatedEvents, null, 2));
    }
  });
});
