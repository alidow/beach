'use client';

import { createContext, useContext } from 'react';
import type { ReactNode } from 'react';
import type { CanvasPoint } from './types';

export type TileMoveReport = {
  tileId: string;
  size: { width: number; height: number };
  originalPosition: CanvasPoint;
  rawPosition: CanvasPoint;
  snappedPosition: CanvasPoint;
  source: 'pointer' | 'keyboard';
};

type CanvasEventsContextValue = {
  reportTileMove: (event: TileMoveReport) => void;
};

const CanvasEventsContext = createContext<CanvasEventsContextValue>({
  reportTileMove: () => {},
});

type CanvasEventsProviderProps = {
  value: CanvasEventsContextValue;
  children: ReactNode;
};

export function CanvasEventsProvider({ value, children }: CanvasEventsProviderProps) {
  return <CanvasEventsContext.Provider value={value}>{children}</CanvasEventsContext.Provider>;
}

export function useCanvasEvents(): CanvasEventsContextValue {
  return useContext(CanvasEventsContext);
}
