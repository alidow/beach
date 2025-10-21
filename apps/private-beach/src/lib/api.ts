export type SessionSummary = {
  session_id: string;
  private_beach_id: string;
  harness_type: string;
  capabilities: string[];
  location_hint?: string | null;
  metadata?: any;
  version: string;
  harness_id: string;
  controller_token?: string | null;
  controller_expires_at_ms?: number | null;
  pending_actions: number;
  pending_unacked: number;
  last_health?: {
    queue_depth: number;
    cpu_load?: number;
    memory_bytes?: number;
    degraded: boolean;
    warnings: string[];
  } | null;
};

export type ControllerLeaseResponse = {
  controller_token: string;
  expires_at_ms: number;
};

export type ViewerCredential = {
  credential_type: string;
  credential: string;
  session_id: string;
  private_beach_id: string;
  issued_at_ms?: number | null;
  expires_at_ms?: number | null;
  passcode?: string | null;
};

function base(baseUrl?: string) {
  if (baseUrl) return baseUrl;
  return process.env.NEXT_PUBLIC_MANAGER_URL || 'http://localhost:8080';
}

function authHeaders(token: string | null) {
  if (!token || token.trim().length === 0) {
    throw new Error('missing manager auth token');
  }
  const trimmed = token.trim();
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  headers['authorization'] = `Bearer ${trimmed}`;
  return headers;
}

export async function listSessions(privateBeachId: string, token: string | null, baseUrl?: string): Promise<SessionSummary[]> {
  const res = await fetch(`${base(baseUrl)}/private-beaches/${privateBeachId}/sessions`, {
    headers: authHeaders(token),
  });
  if (!res.ok) throw new Error(`listSessions failed ${res.status}`);
  return res.json();
}

export async function acquireController(sessionId: string, ttlMs: number | undefined, token: string | null, baseUrl?: string): Promise<ControllerLeaseResponse> {
  const res = await fetch(`${base(baseUrl)}/sessions/${sessionId}/controller/lease`, {
    method: 'POST',
    headers: authHeaders(token),
    body: JSON.stringify({ ttl_ms: ttlMs ?? 30000, reason: 'surfer' }),
  });
  if (!res.ok) throw new Error(`acquireController failed ${res.status}`);
  return res.json();
}

export async function releaseController(sessionId: string, controllerToken: string, token: string | null, baseUrl?: string): Promise<void> {
  const res = await fetch(`${base(baseUrl)}/sessions/${sessionId}/controller/lease`, {
    method: 'DELETE',
    headers: authHeaders(token),
    body: JSON.stringify({ controller_token: controllerToken }),
  });
  if (!res.ok) throw new Error(`releaseController failed ${res.status}`);
}

export async function emergencyStop(sessionId: string, token: string | null, baseUrl?: string, reason?: string): Promise<void> {
  const res = await fetch(`${base(baseUrl)}/sessions/${sessionId}/emergency-stop`, {
    method: 'POST',
    headers: authHeaders(token),
    body: JSON.stringify({ reason: reason || 'user' }),
  });
  if (!res.ok) throw new Error(`emergencyStop failed ${res.status}`);
}

export function eventsSseUrl(sessionId: string, baseUrl?: string, accessToken?: string): string {
  if (!accessToken || accessToken.trim().length === 0) {
    throw new Error('missing manager auth token');
  }
  const t = `?access_token=${encodeURIComponent(accessToken.trim())}`;
  return `${base(baseUrl)}/sessions/${sessionId}/events/stream${t}`;
}

export async function fetchViewerCredential(
  privateBeachId: string,
  sessionId: string,
  token: string | null,
  baseUrl?: string,
): Promise<ViewerCredential> {
  const res = await fetch(
    `${base(baseUrl)}/private-beaches/${privateBeachId}/sessions/${sessionId}/viewer-credential`,
    {
      headers: authHeaders(token),
    },
  );
  if (res.status === 404) {
    throw new Error('viewer credential unavailable');
  }
  if (!res.ok) {
    throw new Error(`fetchViewerCredential failed ${res.status}`);
  }
  return res.json();
}

export async function attachByCode(privateBeachId: string, sessionId: string, code: string, token: string | null, baseUrl?: string) {
  const res = await fetch(`${base(baseUrl)}/private-beaches/${privateBeachId}/sessions/attach-by-code`, {
    method: 'POST',
    headers: authHeaders(token),
    body: JSON.stringify({ session_id: sessionId, code }),
  });
  if (!res.ok) throw new Error(`attachByCode failed ${res.status}`);
  return res.json();
}

export async function attachOwned(privateBeachId: string, ids: string[], token: string | null, baseUrl?: string) {
  const res = await fetch(`${base(baseUrl)}/private-beaches/${privateBeachId}/sessions/attach`, {
    method: 'POST',
    headers: authHeaders(token),
    body: JSON.stringify({ origin_session_ids: ids }),
  });
  if (!res.ok) throw new Error(`attachOwned failed ${res.status}`);
  return res.json();
}

// ---- Private Beaches API ----

export type BeachSummary = { id: string; name: string; slug: string; created_at: number };
export type BeachMeta = BeachSummary & { settings: any };
export type BeachLayoutItem = { id: string; x: number; y: number; w: number; h: number };
export type BeachLayout = {
  preset: 'grid2x2' | 'onePlusThree' | 'focus';
  tiles: string[];
  layout: BeachLayoutItem[];
};

function normalizeLayoutItems(input: unknown): BeachLayoutItem[] {
  if (!Array.isArray(input)) return [];
  const seen = new Set<string>();
  const clean: BeachLayoutItem[] = [];
  for (const raw of input) {
    if (!raw || typeof raw !== 'object') continue;
    const id = typeof (raw as any).id === 'string' ? (raw as any).id.trim() : '';
    if (!id || seen.has(id)) continue;
    const x = Number.isFinite((raw as any).x) ? Math.max(0, Math.floor((raw as any).x)) : null;
    const y = Number.isFinite((raw as any).y) ? Math.max(0, Math.floor((raw as any).y)) : null;
    const w = Number.isFinite((raw as any).w) ? Math.max(1, Math.floor((raw as any).w)) : null;
    const h = Number.isFinite((raw as any).h) ? Math.max(1, Math.floor((raw as any).h)) : null;
    if (x === null || y === null || w === null || h === null) continue;
    clean.push({ id, x, y, w, h });
    seen.add(id);
    if (clean.length >= 12) break;
  }
  return clean;
}

export async function listBeaches(token: string | null, baseUrl?: string): Promise<BeachSummary[]> {
  const res = await fetch(`${base(baseUrl)}/private-beaches`, { headers: authHeaders(token) });
  if (!res.ok) throw new Error(`listBeaches failed ${res.status}`);
  return res.json();
}

export async function createBeach(name: string, slug: string | undefined, token: string | null, baseUrl?: string): Promise<BeachSummary> {
  const res = await fetch(`${base(baseUrl)}/private-beaches`, {
    method: 'POST',
    headers: authHeaders(token),
    body: JSON.stringify({ name, slug }),
  });
  if (!res.ok) throw new Error(`createBeach failed ${res.status}`);
  return res.json();
}

export async function getBeachMeta(id: string, token: string | null, baseUrl?: string): Promise<BeachMeta> {
  const res = await fetch(`${base(baseUrl)}/private-beaches/${id}`, { headers: authHeaders(token) });
  if (res.status === 404) throw new Error('not_found');
  if (!res.ok) throw new Error(`getBeachMeta failed ${res.status}`);
  return res.json();
}

export async function getBeachLayout(id: string, _token: string | null): Promise<BeachLayout> {
  const res = await fetch(`/api/layout/${encodeURIComponent(id)}`);
  if (!res.ok) throw new Error(`getBeachLayout failed ${res.status}`);
  const data = await res.json();
  return {
    preset: (data.preset || 'grid2x2') as BeachLayout['preset'],
    tiles: Array.isArray(data.tiles) ? data.tiles : [],
    layout: normalizeLayoutItems(data.layout),
  };
}

export async function putBeachLayout(id: string, layout: BeachLayout, _token: string | null): Promise<void> {
  const res = await fetch(`/api/layout/${encodeURIComponent(id)}`, {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      preset: layout.preset,
      tiles: layout.tiles,
      layout: normalizeLayoutItems(layout.layout),
    }),
  });
  if (!res.ok) throw new Error(`putBeachLayout failed ${res.status}`);
}

export async function updateBeach(id: string, patch: { name?: string; slug?: string; settings?: any }, token: string | null, baseUrl?: string): Promise<BeachMeta> {
  const res = await fetch(`${base(baseUrl)}/private-beaches/${id}`, {
    method: 'PATCH',
    headers: authHeaders(token),
    body: JSON.stringify(patch),
  });
  if (!res.ok) throw new Error(`updateBeach failed ${res.status}`);
  return res.json();
}
