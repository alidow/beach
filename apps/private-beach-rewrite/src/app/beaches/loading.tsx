import { AppShellTopNav } from '@/components/AppShellTopNav';

export default function BeachesLoading() {
  return (
    <div className="flex min-h-screen flex-col bg-background">
      <AppShellTopNav title="Private Beach" subtitle="Loading your beaches…" />
      <main className="flex-1">
        <div className="mx-auto w-full max-w-5xl px-4 pb-12 pt-6 sm:px-6 lg:px-8">
          <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
            {Array.from({ length: 6 }).map((_, index) => (
              <div key={index} className="h-32 rounded-lg border border-border bg-muted/40" />
            ))}
          </div>
        </div>
      </main>
    </div>
  );
}
