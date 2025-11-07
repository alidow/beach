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
  const { userId, getToken } = safeAuth();
  const isSignedIn = Boolean(userId);

  const template = process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE;
  const { token, source } = await resolveManagerToken(isSignedIn ? getToken : undefined, template);
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
  } else {
    loadError =
      source === 'none'
        ? 'Missing PRIVATE_BEACH_MANAGER_TOKEN. See secret-distribution.md for setup details.'
        : 'Unable to resolve manager auth token. Please sign in again or verify Clerk configuration.';
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
