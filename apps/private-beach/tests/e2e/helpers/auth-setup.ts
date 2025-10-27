import { chromium, FullConfig } from '@playwright/test';
import path from 'path';

/**
 * Global setup to create authenticated session
 * Run this once to create auth state that can be reused by tests
 */
async function globalSetup(config: FullConfig) {
  const authFile = path.join(__dirname, '../.auth/user.json');

  console.log('Setting up authentication...');

  const browser = await chromium.launch();
  const context = await browser.newContext();
  const page = await context.newPage();

  try {
    // Navigate to sign-in
    await page.goto('http://localhost:3000/sign-in');

    console.log('Please sign in manually in the browser window that opened...');
    console.log('Waiting for redirect to /beaches...');

    // Wait for manual sign-in - user completes OAuth flow
    await page.waitForURL('**/beaches**', { timeout: 120000 });

    console.log('Successfully signed in!');

    // Save authenticated state
    await context.storageState({ path: authFile });
    console.log(`Saved authentication state to ${authFile}`);

  } catch (error) {
    console.error('Authentication setup failed:', error);
    throw error;
  } finally {
    await context.close();
    await browser.close();
  }
}

export default globalSetup;
