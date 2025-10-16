import * as React from 'react';
import * as SwitchPrimitives from '@radix-ui/react-switch';
import { cn } from '../../lib/utils';

const Switch = React.forwardRef<
  React.ElementRef<typeof SwitchPrimitives.Root>,
  React.ComponentPropsWithoutRef<typeof SwitchPrimitives.Root>
>(({ className, ...props }, ref) => (
  <SwitchPrimitives.Root
    ref={ref}
    className={cn(
      'peer inline-flex h-6 w-12 shrink-0 cursor-pointer items-center rounded-full border border-[hsl(var(--border))]/70 bg-[hsl(var(--terminal-bezel))]/70 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[hsl(var(--ring))]/60 focus-visible:ring-offset-2 focus-visible:ring-offset-slate-950 disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:bg-[hsl(var(--accent))]',
      className,
    )}
    {...props}
  >
    <SwitchPrimitives.Thumb
      className="pointer-events-none block size-5 translate-x-1 rounded-full bg-slate-950 shadow-[0_3px_6px_rgba(0,0,0,0.45)] transition-transform data-[state=checked]:translate-x-[1.6rem] data-[state=checked]:bg-slate-950"
    />
  </SwitchPrimitives.Root>
));
Switch.displayName = SwitchPrimitives.Root.displayName;

export { Switch };
