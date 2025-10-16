import * as CollapsiblePrimitive from '@radix-ui/react-collapsible';
import * as React from 'react';
import { cn } from '../../lib/utils';

const Collapsible = CollapsiblePrimitive.Root;

const CollapsibleTrigger = CollapsiblePrimitive.Trigger;

const CollapsibleContent = React.forwardRef<
  React.ElementRef<typeof CollapsiblePrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof CollapsiblePrimitive.Content>
>((props, ref) => {
  const { className, children, ...rest } = props;
  return (
    <CollapsiblePrimitive.Content
      ref={ref}
      className={cn(
        'overflow-hidden text-sm transition-all data-[state=closed]:animate-collapsible-up data-[state=open]:animate-collapsible-down',
        className,
      )}
      {...rest}
    >
      {children}
    </CollapsiblePrimitive.Content>
  );
});
CollapsibleContent.displayName = CollapsiblePrimitive.Content.displayName;

export { Collapsible, CollapsibleContent, CollapsibleTrigger };
