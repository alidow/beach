'use client';

import { createContext, useContext, useMemo, type ReactNode } from 'react';

type ManagerTokenContextValue = {
  initialToken: string | null;
};

const ManagerTokenContext = createContext<ManagerTokenContextValue>({ initialToken: null });

type ManagerTokenProviderProps = {
  initialToken?: string | null;
  children: ReactNode;
};

export function ManagerTokenProvider({ initialToken, children }: ManagerTokenProviderProps) {
  const value = useMemo<ManagerTokenContextValue>(() => {
    const trimmed = initialToken?.trim() ?? '';
    return {
      initialToken: trimmed.length > 0 ? trimmed : null,
    };
  }, [initialToken]);

  return <ManagerTokenContext.Provider value={value}>{children}</ManagerTokenContext.Provider>;
}

export function useInitialManagerToken(): ManagerTokenContextValue {
  return useContext(ManagerTokenContext);
}
