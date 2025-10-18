import * as React from 'react';

type DialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title?: string;
  description?: string;
  children?: React.ReactNode;
  footer?: React.ReactNode;
};

export function Dialog({ open, onOpenChange, title, description, children, footer }: DialogProps) {
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50">
      <div className="absolute inset-0 bg-black/30" onClick={() => onOpenChange(false)} />
      <div className="absolute left-1/2 top-1/2 w-[420px] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-neutral-200 bg-white shadow-xl">
        <div className="p-4">
          {title && <h3 className="mb-1 text-sm font-semibold">{title}</h3>}
          {description && <p className="mb-3 text-sm text-neutral-600">{description}</p>}
          {children}
        </div>
        {footer && <div className="border-t border-neutral-200 p-3">{footer}</div>}
      </div>
    </div>
  );
}

