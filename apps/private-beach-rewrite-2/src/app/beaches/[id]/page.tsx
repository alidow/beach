import { notFound } from 'next/navigation';
import { BeachCanvasShell } from '@/features/canvas';
import { AppShellTopNav } from '@/components/AppShellTopNav';
import type { BeachMeta, CanvasLayout } from '@/lib/api';
import { getBeachMeta, getCanvasLayout } from '@/lib/api';
import { resolveManagerBaseUrl, resolveManagerToken, resolveRewriteFlag } from '@/lib/serverSecrets';
import type { Metadata } from 'next';
import { safeAuth } from '@/lib/serverAuth';

type PageProps = {
  params: { id: string };
  searchParams?: Record<string, string | string[] | undefined>;
};

export default async function BeachPage({ params, searchParams }: PageProps) {
  const beachId = params.id;
  const { userId, getToken } = safeAuth();
  const template = process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE;

  const allowedGetToken = userId ? getToken : undefined;
  const { token, source } = await resolveManagerToken(allowedGetToken, template);
  const managerBaseUrl = resolveManagerBaseUrl();
  const rewriteEnabled = resolveRewriteFlag(searchParams);

  if (!token) {
    return (
      <div className="flex min-h-screen flex-col bg-background">
        <AppShellTopNav
          backHref="/beaches"
          title="Private Beach"
          subtitle={
            source === 'none'
              ? 'Manager token missing. Configure PRIVATE_BEACH_MANAGER_TOKEN to load this beach.'
              : 'Sign in to load this beach.'
          }
        />
        <main className="flex-1">
          <div className="mx-auto flex h-full max-w-4xl flex-col items-center justify-center px-4 text-center text-sm text-muted-foreground sm:px-6 lg:px-8">
            {source === 'none'
              ? 'We could not resolve PRIVATE_BEACH_MANAGER_TOKEN. Follow the instructions in docs/private-beach-rewrite/secret-distribution.md and refresh.'
              : 'We could not retrieve your access token. Please sign in again to continue.'}
          </div>
        </main>
      </div>
    );
  }

  let beach: BeachMeta | null = null;
  let layout: CanvasLayout | null = null;
  try {
    beach = await getBeachMeta(beachId, token, managerBaseUrl);
  } catch (error) {
    if (error instanceof Error) {
      const status = (error as any).status ?? null;
      if (error.message === 'not_found') {
        notFound();
      }
      if (status === 409) {
        return (
          <div className="flex h-screen flex-col overflow-hidden bg-transparent">
            <AppShellTopNav backHref="/beaches" title="Private Beach" subtitle={beachId} />
            <main className="flex flex-1 items-center justify-center px-6 text-center text-sm text-slate-400">
              This beach is currently updating its layout. Please wait a moment and try again.
            </main>
          </div>
        );
      }
    }
    throw error;
  }

  try {
    layout = await getCanvasLayout(beach.id, token, managerBaseUrl);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.warn('[rewrite-2] getCanvasLayout failed', { beachId: beach.id, error: message });
    layout = null;
  }

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-transparent" data-private-beach-rewrite={rewriteEnabled ? 'enabled' : 'disabled'}>
      <BeachCanvasShell
        beachId={beach.id}
        beachName={beach.name}
        backHref="/beaches"
        managerUrl={managerBaseUrl}
        managerToken={token}
        initialLayout={layout}
        rewriteEnabled={rewriteEnabled}
        className="flex-1"
      />
    </div>
  );
}

export async function generateMetadata({ params }: PageProps): Promise<Metadata> {
  const { userId, getToken } = safeAuth();
  const template = process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE;

  const allowedGetToken = userId ? getToken : undefined;
  const { token } = await resolveManagerToken(allowedGetToken, template);
  const managerBaseUrl = resolveManagerBaseUrl();

  if (!token) {
    return {
      title: 'Private Beach Rewrite · Beach',
      description: 'Sign in to view this private beach.',
    };
  }

  try {
    const beach = await getBeachMeta(params.id, token, managerBaseUrl);
    return {
      title: `${beach.name} · Private Beach Rewrite`,
      description: `Canvas workspace preview for ${beach.name}.`,
    };
  } catch (error) {
    if (error instanceof Error && error.message === 'not_found') {
      return {
        title: 'Beach not found · Private Beach Rewrite',
        description: 'The requested private beach could not be located.',
      };
    }
    return {
      title: 'Private Beach Rewrite · Beach',
      description: 'View details for this private beach.',
    };
  }
}
