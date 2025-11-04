'use client';

import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from 'react';

export type CanvasUIContextValue = {
  drawerOpen: boolean;
  openDrawer: () => void;
  closeDrawer: () => void;
  toggleDrawer: () => void;
};

const CanvasUIContext = createContext<CanvasUIContextValue | undefined>(undefined);

type CanvasUIProviderProps = {
  initialDrawerOpen?: boolean;
  children: ReactNode;
};

export function CanvasUIProvider({ initialDrawerOpen = true, children }: CanvasUIProviderProps) {
  const [drawerOpen, setDrawerOpen] = useState(initialDrawerOpen);

  const openDrawer = useCallback(() => setDrawerOpen(true), []);
  const closeDrawer = useCallback(() => setDrawerOpen(false), []);
  const toggleDrawer = useCallback(() => setDrawerOpen((previous) => !previous), []);

  const value = useMemo(
    () => ({
      drawerOpen,
      openDrawer,
      closeDrawer,
      toggleDrawer,
    }),
    [drawerOpen, openDrawer, closeDrawer, toggleDrawer],
  );

  return <CanvasUIContext.Provider value={value}>{children}</CanvasUIContext.Provider>;
}

export function useCanvasUI(): CanvasUIContextValue {
  const context = useContext(CanvasUIContext);
  if (!context) {
    throw new Error('useCanvasUI must be used within a CanvasUIProvider');
  }
  return context;
}
