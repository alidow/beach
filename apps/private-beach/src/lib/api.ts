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

function base(baseUrl?: string) {
  if (baseUrl) return baseUrl;
  if (typeof window !== 'undefined') {
    const w = window as any;
    if (w.NEXT_PUBLIC_MANAGER_URL) return w.NEXT_PUBLIC_MANAGER_URL as string;
    const ls = localStorage.getItem('pb.manager');
    if (ls) return ls;
  }
  return process.env.NEXT_PUBLIC_MANAGER_URL || 'http://localhost:3000';
}

function authHeaders(token: string | null) {
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  if (token) headers['authorization'] = `Bearer ${token}`;
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

export function stateSseUrl(sessionId: string, baseUrl?: string, accessToken?: string): string {
  const t = accessToken ? `?access_token=${encodeURIComponent(accessToken)}` : '';
  return `${base(baseUrl)}/sessions/${sessionId}/state/stream${t}`;
}

export function eventsSseUrl(sessionId: string, baseUrl?: string, accessToken?: string): string {
  const t = accessToken ? `?access_token=${encodeURIComponent(accessToken)}` : '';
  return `${base(baseUrl)}/sessions/${sessionId}/events/stream${t}`;
}
