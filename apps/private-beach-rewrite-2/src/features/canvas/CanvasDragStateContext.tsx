'use client';

import { createContext, useContext, type ReactNode } from 'react';

const CanvasDragStateContext = createContext(false);

type CanvasDragStateProviderProps = {
  value: boolean;
  children: ReactNode;
};

export function CanvasDragStateProvider({ value, children }: CanvasDragStateProviderProps) {
  return <CanvasDragStateContext.Provider value={value}>{children}</CanvasDragStateContext.Provider>;
}

export function useIsCanvasDragging(): boolean {
  return useContext(CanvasDragStateContext);
}
