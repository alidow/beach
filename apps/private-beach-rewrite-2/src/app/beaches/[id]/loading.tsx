import { AppShellTopNav } from '@/components/AppShellTopNav';

export default function LoadingBeachPage() {
  return (
    <div className="flex h-screen flex-col overflow-hidden bg-transparent">
      <AppShellTopNav backHref="/beaches" title="Private Beach" subtitle="Setting up your beach…" />
      <main className="flex flex-1 items-center justify-center px-6 text-center text-sm text-slate-400">
        <div className="flex flex-col items-center gap-3">
          <div
            className="h-10 w-10 animate-spin rounded-full border-2 border-border border-t-transparent"
            aria-hidden
          />
          <div className="space-y-1">
            <p className="font-medium text-slate-200">Preparing your beach</p>
            <p className="text-xs text-slate-400">Loading the latest layout and session data…</p>
          </div>
        </div>
      </main>
    </div>
  );
}
