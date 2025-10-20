import * as React from 'react';

type SheetProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  side?: 'right' | 'left';
  children: React.ReactNode;
};

export function Sheet({ open, onOpenChange, side = 'right', children }: SheetProps) {
  React.useEffect(() => {
    if (open) document.body.style.overflow = 'hidden';
    else document.body.style.overflow = '';
    return () => { document.body.style.overflow = ''; };
  }, [open]);

  if (!open) return null;
  const align = side === 'right' ? 'right-0' : 'left-0';
  const borderSide = side === 'right' ? 'border-l' : 'border-r';
  return (
    <div className="fixed inset-0 z-50">
      <div className="absolute inset-0 bg-black/50 backdrop-blur-sm transition-opacity dark:bg-black/70" onClick={() => onOpenChange(false)} />
      <div className={`absolute top-0 ${align} h-full w-[420px] bg-card text-card-foreground shadow-xl ${borderSide} border-border`}>{children}</div>
    </div>
  );
}
