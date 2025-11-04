import { notFound } from 'next/navigation';
import { AppShellTopNav } from '@/components/AppShellTopNav';
import { BeachCanvasShell } from '@/features/canvas';
import type { BeachMeta, SessionSummary } from '@/lib/api';
import { getBeachMeta, listSessions } from '@/lib/api';
import { RewritePreferenceButton } from '@/components/RewritePreferenceButton';
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

  const { token, source } = await resolveManagerToken(userId ? getToken : undefined, template);
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
  try {
    beach = await getBeachMeta(beachId, token, managerBaseUrl);
  } catch (error) {
    if (error instanceof Error && error.message === 'not_found') {
      notFound();
    }
    throw error;
  }

  let sessions: SessionSummary[] = [];
  let sessionError: string | null = null;
  try {
    sessions = await listSessions(beachId, token, managerBaseUrl);
  } catch (error) {
    sessionError = error instanceof Error ? error.message : 'Unable to fetch sessions.';
    sessions = [];
  }

  const metaItems = [
    { label: 'Beach ID', value: beach.id },
    ...(beach.slug ? [{ label: 'Slug', value: beach.slug }] : []),
    { label: 'Sessions', value: String(sessions.length) },
  ];

  return (
    <div
      className="flex min-h-screen flex-col bg-background"
      data-private-beach-rewrite={rewriteEnabled ? 'enabled' : 'disabled'}
    >
      <AppShellTopNav
        backHref="/beaches"
        title={beach.name}
        subtitle="Canvas workspace preview"
        meta={metaItems}
        actions={<RewritePreferenceButton legacyHref={`/beaches/${beach.id}`} />}
      />
      <main className="flex-1">
        <div className="mx-auto flex w-full max-w-6xl flex-1 flex-col gap-6 px-4 pb-12 pt-6 sm:px-6 lg:px-8">
          <section className="grid flex-1 gap-6 lg:grid-cols-[minmax(0,2fr)_minmax(320px,1fr)] lg:gap-4">
            <div className="flex min-h-[520px] flex-col rounded-lg border border-border bg-card/40 p-6">
              <div className="mb-4 flex items-center justify-between">
                <div>
                  <h2 className="text-sm font-semibold text-foreground">Canvas workspace</h2>
                  <p className="text-xs text-muted-foreground">
                    Drag nodes from the catalog to emit placement payloads for WS-D.
                  </p>
                </div>
                <span className="rounded-full bg-secondary px-2 py-1 text-[11px] font-semibold uppercase tracking-wide text-secondary-foreground">
                  WS-C
                </span>
              </div>
              <div className="flex flex-1 flex-col">
                <BeachCanvasShell
                  beachId={beach.id}
                  managerUrl={managerBaseUrl}
                  managerToken={token}
                  rewriteEnabled={rewriteEnabled}
                  className="flex-1"
                />
              </div>
            </div>
            <aside className="flex flex-col gap-4 rounded-lg border border-border bg-card p-6">
              <div>
                <h3 className="text-sm font-semibold text-foreground">Beach details</h3>
                <dl className="mt-3 space-y-2 text-xs text-muted-foreground">
                  <div>
                    <dt className="uppercase tracking-wide text-[11px]">Created</dt>
                    <dd className="font-medium text-foreground">{formatTimestamp(beach.created_at)}</dd>
                  </div>
                  <div>
                    <dt className="uppercase tracking-wide text-[11px]">Identifier</dt>
                    <dd className="font-mono text-[11px]">{beach.id}</dd>
                  </div>
                  {beach.slug ? (
                    <div>
                      <dt className="uppercase tracking-wide text-[11px]">Slug</dt>
                      <dd className="font-mono text-[11px]">{beach.slug}</dd>
                    </div>
                  ) : null}
                </dl>
              </div>
              <div>
                <h3 className="text-sm font-semibold text-foreground">Active sessions</h3>
                {sessionError ? (
                  <p className="mt-2 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-xs text-destructive-foreground">
                    Unable to load sessions: {sessionError}
                  </p>
                ) : sessions.length === 0 ? (
                  <p className="mt-2 text-xs text-muted-foreground">No sessions attached yet.</p>
                ) : (
                  <ul className="mt-3 space-y-2">
                    {sessions.slice(0, 5).map((session) => (
                      <li key={session.session_id} className="rounded-md border border-border bg-background/80 p-3">
                        <div className="text-xs font-semibold text-foreground">{session.session_id}</div>
                        <div className="text-[11px] text-muted-foreground">
                          Harness: {session.harness_type ?? 'n/a'} · Pending actions: {session.pending_actions}
                        </div>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
              <div className="rounded-md border border-dashed border-border bg-background/60 p-3 text-xs text-muted-foreground">
                Node drawer defaults to 320px when expanded with a 16px gutter from the canvas. Coordinate additional
                breakpoints and tile sizing contracts with WS-D via the shared sync log.
              </div>
            </aside>
          </section>
        </div>
      </main>
    </div>
  );
}

function formatTimestamp(value: number | string | undefined) {
  if (!value) return 'Unknown';
  const input = typeof value === 'string' ? Number(value) : value;
  const date = Number.isFinite(input) ? new Date(input as number) : new Date(value);
  if (Number.isNaN(date.getTime())) {
    return 'Unknown';
  }
  return date.toLocaleString();
}

export async function generateMetadata({ params }: PageProps): Promise<Metadata> {
  const { userId, getToken } = safeAuth();
  const template = process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE;

  const { token } = await resolveManagerToken(userId ? getToken : undefined, template);
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
