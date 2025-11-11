'use client';

import { useRef, type ReactNode } from 'react';
import { useIsCanvasDragging } from '@/features/canvas/CanvasDragStateContext';

type DragFreezeBoundaryProps = {
  children: ReactNode;
};

export function DragFreezeBoundary({ children }: DragFreezeBoundaryProps) {
  const freeze = useIsCanvasDragging();
  const snapshotRef = useRef<ReactNode>(children);

  if (!freeze) {
    snapshotRef.current = children;
  }

  return <>{freeze ? snapshotRef.current : children}</>;
}
