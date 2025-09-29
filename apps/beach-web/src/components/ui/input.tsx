import * as React from 'react';
import { cn } from '../../lib/utils';

const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(
  ({ className, type, ...props }, ref) => {
    return (
      <input
        type={type}
        className={cn(
          'flex h-11 w-full rounded-lg border border-[hsl(var(--border))]/70 bg-[hsl(var(--terminal-bezel))]/70 px-4 text-sm text-[hsl(var(--foreground))] shadow-inner transition-colors placeholder:text-[hsl(var(--muted-foreground))]/80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[hsl(var(--ring))]/60 focus-visible:ring-offset-2 focus-visible:ring-offset-slate-950 disabled:cursor-not-allowed disabled:opacity-50',
          className,
        )}
        ref={ref}
        spellCheck={type === 'password' ? false : undefined}
        {...props}
      />
    );
  },
);
Input.displayName = 'Input';

export { Input };
