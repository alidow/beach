import { AppShellTopNav } from '@/components/AppShellTopNav';
import { BeachesList } from '@/components/BeachesList';
import type { BeachSummary } from '@/lib/api';
import { listBeaches } from '@/lib/api';
import { resolveManagerBaseUrl, resolveManagerToken } from '@/lib/serverSecrets';
import { safeAuth } from '@/lib/serverAuth';
import { CreateBeachButton } from '@/components/CreateBeachButton';

export const metadata = {
  title: 'Private Beach Rewrite · Beaches',
  description: 'Browse all private beaches and launch the new canvas shell.',
} as const;

export default async function BeachesPage() {
  const { userId, getToken } = await safeAuth();
  const isSignedIn = Boolean(userId);
  const allowedGetToken = typeof getToken === 'function' ? getToken : undefined;

  const template = process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE;
  const { token, source } = await resolveManagerToken(allowedGetToken, template, {
    isAuthenticated: isSignedIn,
  });
  const managerBaseUrl = resolveManagerBaseUrl();

  let beaches: BeachSummary[] = [];
  let loadError: string | null = null;
  if (token) {
    try {
      beaches = await listBeaches(token, managerBaseUrl);
    } catch (error) {
      loadError = error instanceof Error ? error.message : 'Unknown error occurred while loading beaches.';
      beaches = [];
    }
  } else if (source === 'unauthenticated') {
    loadError = 'Sign in to view your private beaches.';
  } else if (source === 'exchange_error') {
    loadError =
      'Unable to mint a Beach Gate token. Ensure Clerk sessions are valid and Beach Gate is reachable (PRIVATE_BEACH_GATE_URL).';
  } else if (source === 'none') {
    loadError = 'Missing PRIVATE_BEACH_MANAGER_TOKEN. See secret-distribution.md for setup details.';
  } else {
    loadError = 'Unable to resolve manager auth token. Please sign in again or verify Clerk configuration.';
  }

  return (
    <div className="flex min-h-screen flex-col bg-background">
      <AppShellTopNav
        title="Private Beach"
        subtitle="Select a beach to launch the new canvas shell."
        actions={<CreateBeachButton />}
      />
      <main className="flex-1">
        <div className="mx-auto w-full max-w-5xl px-4 pb-12 pt-6 sm:px-6 lg:px-8">
          <BeachesList beaches={beaches} error={loadError} />
        </div>
      </main>
    </div>
  );
}
