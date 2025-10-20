import { createContext, ReactNode, useContext } from 'react';

export type ManagerSettings = {
  managerUrl: string;
  roadUrl: string;
  token: string;
};

export type BeachSettingsContextValue = {
  manager: ManagerSettings;
  updateManager: (partial: Partial<ManagerSettings>) => Promise<void>;
  saving: boolean;
};

const BeachSettingsContext = createContext<BeachSettingsContextValue | null>(null);

type ProviderProps = {
  value: BeachSettingsContextValue;
  children: ReactNode;
};

export function BeachSettingsProvider({ value, children }: ProviderProps) {
  return <BeachSettingsContext.Provider value={value}>{children}</BeachSettingsContext.Provider>;
}

export function useBeachManagerSettings(): BeachSettingsContextValue {
  const ctx = useContext(BeachSettingsContext);
  if (!ctx) {
    throw new Error('useBeachManagerSettings must be used within a BeachSettingsProvider');
  }
  return ctx;
}

