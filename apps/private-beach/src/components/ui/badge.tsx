import * as React from 'react';

type BadgeProps = {
  children: React.ReactNode;
  variant?: 'default' | 'success' | 'warning' | 'danger' | 'muted';
  className?: string;
};

export function Badge({ children, variant = 'default', className = '' }: BadgeProps) {
  let styles = 'bg-neutral-900 text-white';
  if (variant === 'success') styles = 'bg-emerald-600 text-white';
  if (variant === 'warning') styles = 'bg-amber-500 text-white';
  if (variant === 'danger') styles = 'bg-red-600 text-white';
  if (variant === 'muted') styles = 'bg-neutral-200 text-neutral-800';
  return <span className={`inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium ${styles} ${className}`}>{children}</span>;
}

