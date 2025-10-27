import { Page } from '@playwright/test';

/**
 * Sign in to Clerk with username and password
 */
export async function signInWithClerk(
  page: Page,
  username: string,
  password: string
): Promise<void> {
  console.log(`Signing in as ${username}...`);

  // Navigate to sign-in page
  await page.goto('http://localhost:3000/sign-in', { waitUntil: 'networkidle' });

  // Check if already signed in
  if (page.url().includes('/beaches')) {
    console.log('Already signed in');
    return;
  }

  try {
    // Wait for Clerk form to load
    await page.waitForTimeout(2000);
    console.log('Clerk sign-in page loaded');

    // Look for identifier input (email or username)
    const emailField = page.locator('input[name="identifier"]').first();
    await emailField.waitFor({ state: 'visible', timeout: 15000 });

    // Fill username
    await emailField.fill(username);
    console.log('Filled username');

    // Wait a moment for input to register
    await page.waitForTimeout(500);

    // Click Continue button (exact text match to avoid OAuth buttons)
    const continueButton = page.getByRole('button', { name: 'Continue', exact: true });
    await continueButton.click();
    console.log('Clicked continue');

    // Wait for password field - it should become enabled
    await page.waitForTimeout(2000);

    // Look for enabled password field
    const passwordField = page.locator('input[type="password"]').first();
    await passwordField.waitFor({ state: 'visible', timeout: 15000 });
    console.log('Password field is ready');

    // Fill password
    await passwordField.fill(password);
    console.log('Filled password');

    // Wait a moment for input to register
    await page.waitForTimeout(500);

    // Click Continue/Sign in button
    const signInButton = page.getByRole('button', { name: /Continue|Sign in/i });
    await signInButton.click();
    console.log('Clicked sign in');

    // Wait for redirect to /beaches
    await page.waitForURL('**/beaches**', { timeout: 30000 });
    console.log('Successfully signed in');

  } catch (error) {
    console.error('Sign-in error:', error);
    console.log('Current URL:', page.url());

    // Take screenshot for debugging
    try {
      await page.screenshot({ path: 'test-results/clerk-auth-error.png' });
      console.log('Screenshot saved to test-results/clerk-auth-error.png');
    } catch (screenshotError) {
      console.log('Could not capture screenshot');
    }

    throw error;
  }
}

/**
 * Get Clerk session token
 */
export async function getClerkToken(
  page: Page,
  template: string = 'private-beach-manager'
): Promise<string> {
  console.log(`Getting Clerk token with template: ${template}`);

  // Extract token via Clerk API
  const token = await page.evaluate(async (templateName) => {
    // @ts-ignore - Clerk is available globally
    if (!window.Clerk?.session) {
      throw new Error('Clerk session not available');
    }
    // @ts-ignore
    return await window.Clerk.session.getToken({ template: templateName });
  }, template);

  if (!token) {
    throw new Error('Failed to get Clerk token');
  }

  console.log('Successfully retrieved Clerk token');
  return token;
}

/**
 * Save authentication state to a file for reuse
 */
export async function saveAuthState(page: Page, path: string): Promise<void> {
  await page.context().storageState({ path });
  console.log(`Saved auth state to ${path}`);
}

/**
 * Load authentication state from a file
 */
export async function loadAuthState(page: Page, path: string): Promise<void> {
  // This should be done via browser context before creating the page
  console.log(`Load auth state from ${path} via context.storageState()`);
}
