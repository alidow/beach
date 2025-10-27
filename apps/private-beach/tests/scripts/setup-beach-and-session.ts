#!/usr/bin/env npx tsx
/**
 * Automates beach creation and session addition for Private Beach E2E tests.
 * This eliminates the manual step documented in the test README.
 */

import { chromium } from '@playwright/test';

interface SetupResult {
  beachId: string;
  sessionId: string;
  passcode: string;
}

async function setupBeachAndSession(): Promise<SetupResult> {
  const sessionId = process.env.BEACH_TEST_SESSION_ID;
  const passcode = process.env.BEACH_TEST_PASSCODE;
  const privateBeachUrl = process.env.BEACH_TEST_PRIVATE_BEACH_URL || 'http://localhost:3000';

  if (!sessionId || !passcode) {
    throw new Error('BEACH_TEST_SESSION_ID and BEACH_TEST_PASSCODE must be set');
  }

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext();
  const page = await context.newPage();

  try {
    console.log('🏖️  Navigating to Private Beach...');
    await page.goto(privateBeachUrl);
    await page.waitForLoadState('networkidle');

    // Check if there's already a beach we can use
    const beachLinks = page.locator('a[href^="/beaches/"]');
    const count = await beachLinks.count();

    let beachId: string;

    if (count > 0) {
      console.log(`✅ Found ${count} existing beach(es), using the first one`);
      const firstBeachLink = beachLinks.first();
      const href = await firstBeachLink.getAttribute('href');
      beachId = href?.split('/beaches/')[1] || '';
      await firstBeachLink.click();
    } else {
      console.log('🆕 Creating new beach...');
      // Look for "Create Beach" or similar button
      const createButton = page.locator('button:has-text("Create"), button:has-text("New Beach"), a:has-text("Create Beach")').first();
      await createButton.click();

      // Wait for navigation to beach page
      await page.waitForURL(/\/beaches\/[^/]+/);
      const url = page.url();
      beachId = url.split('/beaches/')[1].split(/[?#]/)[0];
      console.log(`✅ Created beach: ${beachId}`);
    }

    // Now add the session
    console.log('➕ Adding session to beach...');

    // Check if session already exists
    const existingSession = page.locator(`[data-session-id="${sessionId}"]`);
    const sessionExists = await existingSession.count() > 0;

    if (sessionExists) {
      console.log('✅ Session already added to beach');
    } else {
      // Look for "Add Session" button or input
      const addSessionButton = page.locator('button:has-text("Add"), button:has-text("Join"), input[placeholder*="session"], input[placeholder*="Session"]').first();

      if (await addSessionButton.count() > 0) {
        // If it's an input field, type the session ID
        const tagName = await addSessionButton.evaluate(el => el.tagName.toLowerCase());
        if (tagName === 'input') {
          await addSessionButton.fill(sessionId);
          // Look for submit button
          const submitButton = page.locator('button:has-text("Join"), button:has-text("Add"), button[type="submit"]').first();
          await submitButton.click();
        } else {
          await addSessionButton.click();
          // Wait for input field to appear
          const sessionInput = page.locator('input[placeholder*="session"], input[placeholder*="Session"], input[type="text"]').first();
          await sessionInput.fill(sessionId);

          // Enter passcode if prompted
          const passcodeInput = page.locator('input[placeholder*="passcode"], input[placeholder*="Passcode"], input[type="password"]').first();
          if (await passcodeInput.count() > 0) {
            await passcodeInput.fill(passcode);
          }

          // Submit
          const submitButton = page.locator('button:has-text("Join"), button:has-text("Connect"), button[type="submit"]').first();
          await submitButton.click();
        }

        // Wait for session to appear
        await page.waitForSelector(`[data-session-id="${sessionId}"]`, { timeout: 10000 });
        console.log('✅ Session added successfully');
      }
    }

    console.log(`\n🎉 Setup complete!`);
    console.log(`Beach ID: ${beachId}`);
    console.log(`Session ID: ${sessionId}`);
    console.log(`Passcode: ${passcode}`);

    await browser.close();

    return {
      beachId,
      sessionId,
      passcode,
    };
  } catch (error) {
    await browser.close();
    throw error;
  }
}

// Run if executed directly
if (require.main === module) {
  setupBeachAndSession()
    .then(result => {
      console.log(`\n📋 Environment variables for testing:`);
      console.log(`export BEACH_ID="${result.beachId}"`);
      console.log(`export BEACH_TEST_SESSION_ID="${result.sessionId}"`);
      console.log(`export BEACH_TEST_PASSCODE="${result.passcode}"`);
      process.exit(0);
    })
    .catch(error => {
      console.error('❌ Setup failed:', error);
      process.exit(1);
    });
}

export { setupBeachAndSession };
