import { AppShellTopNav } from '@/components/AppShellTopNav';

export default function BeachLoading() {
  return (
    <div className="flex min-h-screen flex-col bg-background">
      <AppShellTopNav backHref="/beaches" title="Loading beach…" />
      <main className="flex-1">
        <div className="mx-auto flex w-full max-w-6xl flex-1 flex-col gap-6 px-4 pb-12 pt-6 sm:px-6 lg:px-8">
          <div className="grid flex-1 gap-6 lg:grid-cols-[minmax(0,2fr)_minmax(320px,1fr)]">
            <div className="min-h-[480px] rounded-lg border border-border bg-muted/40" />
            <div className="rounded-lg border border-border bg-muted/30" />
          </div>
        </div>
      </main>
    </div>
  );
}
