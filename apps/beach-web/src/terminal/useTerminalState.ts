import { useMemo, useSyncExternalStore } from 'react';
import type { TerminalGridSnapshot } from './gridStore';
import { TerminalGridStore } from './gridStore';

export function createTerminalStore(initialCols = 0): TerminalGridStore {
  return new TerminalGridStore(initialCols);
}

export function useTerminalSnapshot(store: TerminalGridStore): TerminalGridSnapshot {
  const subscribe = useMemo(() => store.subscribe.bind(store), [store]);
  return useSyncExternalStore(subscribe, () => store.getSnapshot(), () => store.getSnapshot());
}
