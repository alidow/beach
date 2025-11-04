'use client';

import { useDroppable } from '@dnd-kit/core';
import { forwardRef, useCallback, type CSSProperties, type HTMLAttributes, type MutableRefObject, type ReactNode } from 'react';

export const CANVAS_SURFACE_ID = 'canvas-surface';

type CanvasSurfaceProps = {
  children?: ReactNode;
} & Omit<HTMLAttributes<HTMLDivElement>, 'children'>;

export const CanvasSurface = forwardRef<HTMLDivElement, CanvasSurfaceProps>(function CanvasSurface(
  { children, className, ...rest },
  forwardedRef,
) {
  const { isOver, setNodeRef } = useDroppable({
    id: CANVAS_SURFACE_ID,
  });

  const mergedRef = useCallback(
    (node: HTMLDivElement | null) => {
      setNodeRef(node);
      if (!forwardedRef) return;
      if (typeof forwardedRef === 'function') {
        forwardedRef(node);
      } else {
        (forwardedRef as MutableRefObject<HTMLDivElement | null>).current = node;
      }
    },
    [forwardedRef, setNodeRef],
  );

  const classes = [
    'relative flex-1 overflow-hidden rounded-xl border border-border bg-card/60 shadow-inner transition-all',
    'min-h-[640px]',
    isOver ? 'ring-2 ring-primary ring-offset-2 ring-offset-background' : '',
    className ?? '',
  ]
    .filter(Boolean)
    .join(' ');

  const gridStyle: CSSProperties = {
    backgroundImage:
      'linear-gradient(to right, rgba(148, 163, 184, 0.12) 1px, transparent 1px), linear-gradient(to bottom, rgba(148, 163, 184, 0.12) 1px, transparent 1px)',
    backgroundSize: '8px 8px',
  };

  return (
    <div ref={mergedRef} className={classes} style={gridStyle} {...rest}>
      <div className="pointer-events-none absolute inset-0 bg-gradient-to-b from-transparent via-transparent to-black/5" />
      <div className="relative h-full w-full">{children}</div>
    </div>
  );
});
