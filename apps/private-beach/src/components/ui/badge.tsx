import * as React from 'react';

type BadgeProps = {
  children: React.ReactNode;
  variant?: 'default' | 'success' | 'warning' | 'danger' | 'muted';
  className?: string;
};

export function Badge({ children, variant = 'default', className = '' }: BadgeProps) {
  let styles = 'bg-secondary text-secondary-foreground';
  if (variant === 'success') styles = 'bg-emerald-500/90 text-emerald-950 dark:text-emerald-50';
  if (variant === 'warning') styles = 'bg-amber-400/90 text-amber-950';
  if (variant === 'danger') styles = 'bg-destructive text-destructive-foreground';
  if (variant === 'muted') styles = 'bg-muted text-muted-foreground';
  return <span className={`inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium ${styles} ${className}`}>{children}</span>;
}
