export type PrivateBeach = {
  id: string;
  name: string;
  managerUrl: string;
  token: string | null;
  createdAt: number;
};

const LS_KEY = 'pb.beaches';

function readAll(): PrivateBeach[] {
  if (typeof window === 'undefined') return [];
  try {
    const raw = localStorage.getItem(LS_KEY);
    if (!raw) return [];
    const arr = JSON.parse(raw) as PrivateBeach[];
    return Array.isArray(arr) ? arr : [];
  } catch {
    return [];
  }
}

function writeAll(list: PrivateBeach[]) {
  if (typeof window === 'undefined') return;
  localStorage.setItem(LS_KEY, JSON.stringify(list));
}

export function listBeaches(): PrivateBeach[] {
  return readAll().sort((a, b) => b.createdAt - a.createdAt);
}

export function getBeach(id: string): PrivateBeach | null {
  return readAll().find((b) => b.id === id) || null;
}

export function upsertBeach(input: Omit<PrivateBeach, 'createdAt'> & { createdAt?: number }): PrivateBeach {
  const list = readAll();
  const idx = list.findIndex((b) => b.id === input.id);
  const next: PrivateBeach = {
    id: input.id,
    name: input.name,
    managerUrl: input.managerUrl,
    token: input.token ?? null,
    createdAt: input.createdAt ?? Date.now(),
  };
  if (idx >= 0) list[idx] = next; else list.push(next);
  writeAll(list);
  return next;
}

export function deleteBeach(id: string) {
  const list = readAll().filter((b) => b.id !== id);
  writeAll(list);
}

export function ensureId(id?: string): string {
  if (id && id.trim()) return id;
  if (typeof window !== 'undefined' && 'crypto' in window && (window.crypto as any).randomUUID) {
    return (window.crypto as any).randomUUID();
  }
  // Fallback UUID v4-ish
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === 'x' ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

// Per-beach layout persistence
export type BeachLayout = {
  // ordered session ids in the canvas
  tiles: string[];
  // selected preset id
  preset: 'grid2x2' | 'onePlusThree' | 'focus';
};

export function loadLayout(beachId: string): BeachLayout {
  if (typeof window === 'undefined') return { tiles: [], preset: 'grid2x2' };
  try {
    const raw = localStorage.getItem(`pb.layout.${beachId}`);
    if (!raw) return { tiles: [], preset: 'grid2x2' };
    const obj = JSON.parse(raw) as BeachLayout;
    return {
      tiles: Array.isArray(obj.tiles) ? obj.tiles : [],
      preset: obj.preset || 'grid2x2',
    };
  } catch {
    return { tiles: [], preset: 'grid2x2' };
  }
}

export function saveLayout(beachId: string, layout: BeachLayout) {
  if (typeof window === 'undefined') return;
  localStorage.setItem(`pb.layout.${beachId}`, JSON.stringify(layout));
}

