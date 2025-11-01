export type ViewerTileCounters = {
  started: number;
  completed: number;
  retries: number;
  failures: number;
  disposed: number;
};

const STORE_KEY = '__private_beach_viewer_counters__';

function getStore(): Map<string, ViewerTileCounters> {
  const globalObj = globalThis as Record<string, unknown>;
  const existing = globalObj[STORE_KEY];
  if (existing && existing instanceof Map) {
    return existing as Map<string, ViewerTileCounters>;
  }
  const created = new Map<string, ViewerTileCounters>();
  globalObj[STORE_KEY] = created;
  return created;
}

const counters = getStore();

function ensure(tileId: string): ViewerTileCounters {
  let entry = counters.get(tileId);
  if (!entry) {
    entry = {
      started: 0,
      completed: 0,
      retries: 0,
      failures: 0,
      disposed: 0,
    };
    counters.set(tileId, entry);
  }
  return entry;
}

export function incrementViewerCounter(tileId: string, key: keyof ViewerTileCounters) {
  const entry = ensure(tileId);
  entry[key] += 1;
}

export function getViewerCounters(tileId: string): ViewerTileCounters | undefined {
  const entry = counters.get(tileId);
  if (!entry) {
    return undefined;
  }
  return { ...entry };
}

export function getAllViewerCounters(): Array<{ tileId: string; counters: ViewerTileCounters }> {
  return Array.from(counters.entries()).map(([tileId, entry]) => ({
    tileId,
    counters: { ...entry },
  }));
}

export function resetViewerCounters() {
  counters.clear();
}
