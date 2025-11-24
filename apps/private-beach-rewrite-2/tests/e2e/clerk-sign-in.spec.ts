import { expect, test } from '@playwright/test';
import { clerk, clerkSetup } from '@clerk/testing/playwright';

const clerkUser = process.env.CLERK_USER || 'test@beach.sh';
const clerkPass = process.env.CLERK_PASS || 'h3llo Beach';
const baseUrl = (process.env.PRIVATE_BEACH_REWRITE_URL || 'http://localhost:3003').replace(/\/$/, '');
const beachesUrl = `${baseUrl}/beaches`;
const hasClerkSecrets = Boolean(process.env.CLERK_SECRET_KEY) && Boolean(process.env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY);

test.beforeAll(async () => {
  test.skip(!hasClerkSecrets, 'Clerk testing helpers need CLERK_SECRET_KEY and NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY');
  await clerkSetup({
    secretKey: process.env.CLERK_SECRET_KEY,
    publishableKey: process.env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY,
  });
});

test('Clerk test user can sign in to rewrite-2 beaches list', async ({ page }) => {
  test.skip(!hasClerkSecrets, 'Clerk testing helpers need CLERK_SECRET_KEY and NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY');

  // Load a page that initializes Clerk before invoking the helper.
  await page.goto(baseUrl, { waitUntil: 'domcontentloaded' });

  await clerk.signIn({
    page,
    signInParams: { strategy: 'password', identifier: clerkUser, password: clerkPass },
  });

  await page.goto(beachesUrl, { waitUntil: 'domcontentloaded' });
  await expect(page.getByRole('button', { name: /sign in/i })).toBeHidden({ timeout: 10_000 });
  await expect(page.getByRole('button', { name: /new beach/i })).toBeVisible({ timeout: 10_000 });
});
