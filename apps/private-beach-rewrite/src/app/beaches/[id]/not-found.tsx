import Link from 'next/link';
import { AppShellTopNav } from '@/components/AppShellTopNav';

export default function BeachNotFound() {
  return (
    <div className="flex min-h-screen flex-col bg-background">
      <AppShellTopNav backHref="/beaches" title="Beach unavailable" subtitle="We could not find that beach id." />
      <main className="flex-1">
        <div className="mx-auto flex h-full max-w-4xl flex-col items-center justify-center gap-4 px-4 text-center sm:px-6 lg:px-8">
          <p className="text-sm text-muted-foreground">
            The requested beach does not exist or you no longer have access to it.
          </p>
          <Link href="/beaches" className="text-sm font-semibold text-primary hover:underline">
            Return to beaches list
          </Link>
        </div>
      </main>
    </div>
  );
}
