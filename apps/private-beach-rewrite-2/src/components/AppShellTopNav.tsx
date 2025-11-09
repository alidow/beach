'use client';

import Link from 'next/link';
import { SignedIn, SignedOut, SignInButton, UserButton } from '@clerk/nextjs';
import { Button } from '../../../private-beach/src/components/ui/button';
import { ThemeToggleButton } from './ThemeToggleButton';

type MetaItem = {
  label: string;
  value: string;
};

type Props = {
  backHref?: string;
  backLabel?: string;
  title: string;
  subtitle?: string;
  meta?: MetaItem[];
  actions?: React.ReactNode;
};

export function AppShellTopNav({ backHref, backLabel = 'Back', title, subtitle, meta = [], actions }: Props) {
  return (
    <header className="border-b border-border bg-background/90 backdrop-blur supports-[backdrop-filter]:bg-background/60">
      <div className="mx-auto flex h-16 max-w-6xl items-center justify-between px-4 sm:px-6 lg:px-8">
        <div className="flex flex-1 items-center gap-4">
          <div className="flex items-center gap-3">
            {backHref ? (
              <Link href={backHref} className="text-sm font-medium text-muted-foreground hover:text-foreground">
                ← {backLabel}
              </Link>
            ) : (
              <Link href="/beaches" className="text-sm font-semibold">
                Private Beach
              </Link>
            )}
            <div>
              <div className="text-sm font-semibold leading-tight text-foreground">{title}</div>
              {subtitle ? <div className="text-xs text-muted-foreground">{subtitle}</div> : null}
            </div>
          </div>
          {meta.length > 0 ? (
            <dl className="hidden items-center gap-4 text-xs text-muted-foreground md:flex">
              {meta.map((item) => (
                <div key={item.label} className="flex items-center gap-1">
                  <dt className="uppercase tracking-wide">{item.label}</dt>
                  <dd className="font-mono text-[11px] text-foreground">{item.value}</dd>
                </div>
              ))}
            </dl>
          ) : null}
        </div>
        <div className="flex items-center gap-2">
          {actions}
          <ThemeToggleButton />
          <SignedIn>
            <UserButton afterSignOutUrl="/sign-in" />
          </SignedIn>
          <SignedOut>
            <SignInButton mode="modal">
              <Button variant="outline" size="sm">
                Sign in
              </Button>
            </SignInButton>
          </SignedOut>
        </div>
      </div>
    </header>
  );
}
