export type SessionSummary = {
  session_id: string;
  private_beach_id: string;
  harness_type: string;
  capabilities: string[];
  location_hint?: string | null;
  metadata?: any;
  last_state?: unknown;
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

export const SESSION_ROLE_OPTIONS = ['agent', 'application'] as const;
export type SessionRole = (typeof SESSION_ROLE_OPTIONS)[number];

export const CONTROLLER_UPDATE_CADENCE_OPTIONS = ['fast', 'balanced', 'slow'] as const;
export type ControllerUpdateCadence = (typeof CONTROLLER_UPDATE_CADENCE_OPTIONS)[number];

export type PairingTransportStatus = {
  transport: 'fast_path' | 'http_fallback' | 'pending';
  last_event_ms?: number | null;
  last_error?: string | null;
  latency_ms?: number | null;
};

export type ControllerPairing = {
  pairing_id: string;
  controller_session_id: string;
  child_session_id: string;
  prompt_template?: string | null;
  update_cadence: ControllerUpdateCadence;
  transport_status?: PairingTransportStatus | null;
  created_at_ms?: number | null;
  updated_at_ms?: number | null;
};

export type ControllerLeaseResponse = {
  controller_token: string;
  expires_at_ms: number;
};

export type ControllerEvent = {
  id: string;
  event_type: string;
  controller_token?: string | null;
  timestamp_ms: number;
  reason?: string | null;
  controller_account_id?: string | null;
  issued_by_account_id?: string | null;
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

export async function fetchViewerCredential(
  privateBeachId: string,
  sessionId: string,
  token: string | null,
  baseUrl?: string,
): Promise<ViewerCredential> {
  if (typeof window !== 'undefined') {
    console.info('[api] fetchViewerCredential request', {
      privateBeachId,
      sessionId,
      baseUrl: baseUrl ?? base(baseUrl),
      hasToken: Boolean(token && token.trim().length > 0),
    });
  }
  const res = await fetch(
    `${base(baseUrl)}/private-beaches/${privateBeachId}/sessions/${sessionId}/viewer-credential`,
    {
      headers: authHeaders(token),
    },
  );
  if (typeof window !== 'undefined') {
    console.info('[api] fetchViewerCredential response', {
      privateBeachId,
      sessionId,
      status: res.status,
    });
  }
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

export async function fetchControllerEvents(
  sessionId: string,
  token: string | null,
  baseUrl?: string,
  params?: { event_type?: string; since_ms?: number; limit?: number },
): Promise<ControllerEvent[]> {
  const search = new URLSearchParams();
  if (params?.event_type) {
    search.set('event_type', params.event_type);
  }
  if (typeof params?.since_ms === 'number') {
    search.set('since_ms', String(params.since_ms));
  }
  if (typeof params?.limit === 'number') {
    search.set('limit', String(params.limit));
  }
  const qs = search.toString();
  const url = `${base(baseUrl)}/sessions/${sessionId}/controller-events${qs ? `?${qs}` : ''}`;
  const res = await fetch(url, {
    headers: authHeaders(token),
  });
  if (!res.ok) throw new Error(`fetchControllerEvents failed ${res.status}`);
  return res.json();
}

// ---- Private Beaches API ----

export type BeachSummary = { id: string; name: string; slug: string; created_at: number };
export type BeachMeta = BeachSummary & { settings: any };
export type BeachLayoutItem = {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  widthPx?: number;
  heightPx?: number;
  zoom?: number;
  locked?: boolean;
  toolbarPinned?: boolean;
  gridCols?: number;
  rowHeightPx?: number;
  layoutVersion?: number;
};
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
    const item: BeachLayoutItem = { id, x, y, w, h };
    const widthPx = Number.isFinite((raw as any).widthPx) ? Math.max(0, Math.round((raw as any).widthPx)) : null;
    const heightPx = Number.isFinite((raw as any).heightPx) ? Math.max(0, Math.round((raw as any).heightPx)) : null;
    const zoom = Number.isFinite((raw as any).zoom) ? Math.max(0.05, Math.min(4, Number((raw as any).zoom))) : null;
    const locked = typeof (raw as any).locked === 'boolean' ? (raw as any).locked : null;
    const toolbarPinned = typeof (raw as any).toolbarPinned === 'boolean' ? (raw as any).toolbarPinned : null;
    const gridCols = Number.isFinite((raw as any).gridCols)
      ? Math.max(1, Math.floor((raw as any).gridCols))
      : null;
    const rowHeightPx = Number.isFinite((raw as any).rowHeightPx)
      ? Math.max(1, Math.round((raw as any).rowHeightPx))
      : null;
    const layoutVersion = Number.isFinite((raw as any).layoutVersion)
      ? Math.max(0, Math.floor((raw as any).layoutVersion))
      : null;
    if (widthPx !== null) item.widthPx = widthPx;
    if (heightPx !== null) item.heightPx = heightPx;
    if (zoom !== null) item.zoom = zoom;
    if (locked !== null) item.locked = locked;
    if (toolbarPinned !== null) item.toolbarPinned = toolbarPinned;
    if (gridCols !== null) item.gridCols = gridCols;
    if (rowHeightPx !== null) item.rowHeightPx = rowHeightPx;
    if (layoutVersion !== null) item.layoutVersion = layoutVersion;
    clean.push(item);
    seen.add(id);
  }
  return clean;
}

export async function listBeaches(token: string | null, baseUrl?: string): Promise<BeachSummary[]> {
  const res = await fetch(`${base(baseUrl)}/private-beaches`, { headers: authHeaders(token) });
  if (!res.ok) throw new Error(`listBeaches failed ${res.status}`);
  return res.json();
}

function normalizeTransportStatus(raw: any): PairingTransportStatus | null {
  if (!raw || typeof raw !== 'object') {
    return null;
  }
  const transport = typeof raw.transport === 'string' ? raw.transport : null;
  if (transport !== 'fast_path' && transport !== 'http_fallback' && transport !== 'pending') {
    return null;
  }
  const lastEventMs = Number((raw as any).last_event_ms);
  const latencyMs = Number((raw as any).latency_ms);
  const status: PairingTransportStatus = {
    transport,
  };
  if (Number.isFinite(lastEventMs)) {
    status.last_event_ms = lastEventMs;
  }
  if (Number.isFinite(latencyMs)) {
    status.latency_ms = latencyMs;
  }
  if (typeof (raw as any).last_error === 'string' && (raw as any).last_error.trim().length > 0) {
    status.last_error = (raw as any).last_error;
  }
  return status;
}

export function normalizeControllerPairing(raw: any): ControllerPairing {
  if (!raw || typeof raw !== 'object') {
    throw new Error('invalid controller pairing payload');
  }
  const controllerId = typeof raw.controller_session_id === 'string' ? raw.controller_session_id : '';
  const childId = typeof raw.child_session_id === 'string' ? raw.child_session_id : '';
  if (!controllerId || !childId) {
    throw new Error('controller pairing missing session identifiers');
  }
  const pairingId =
    typeof raw.pairing_id === 'string' && raw.pairing_id.trim().length > 0
      ? raw.pairing_id
      : `${controllerId}|${childId}`;
  const cadenceRaw = typeof raw.update_cadence === 'string' ? raw.update_cadence : '';
  const cadence = CONTROLLER_UPDATE_CADENCE_OPTIONS.includes(cadenceRaw as ControllerUpdateCadence)
    ? (cadenceRaw as ControllerUpdateCadence)
    : 'balanced';
  const createdAt = Number(raw.created_at_ms);
  const updatedAt = Number(raw.updated_at_ms);
  const promptTemplate =
    typeof raw.prompt_template === 'string'
      ? raw.prompt_template
      : raw.prompt_template === null
        ? null
        : undefined;
  const pairing: ControllerPairing = {
    pairing_id: pairingId,
    controller_session_id: controllerId,
    child_session_id: childId,
    update_cadence: cadence,
  };
  if (promptTemplate !== undefined) {
    pairing.prompt_template =
      typeof promptTemplate === 'string' && promptTemplate.trim().length > 0 ? promptTemplate : null;
  }
  const status = normalizeTransportStatus((raw as any).transport_status);
  if (status) {
    pairing.transport_status = status;
  }
  if (Number.isFinite(createdAt)) {
    pairing.created_at_ms = createdAt;
  }
  if (Number.isFinite(updatedAt)) {
    pairing.updated_at_ms = updatedAt;
  }
  return pairing;
}

export function sortControllerPairings(list: ControllerPairing[]): ControllerPairing[] {
  return list
    .slice()
    .sort((a, b) => {
      const controllerOrder = a.controller_session_id.localeCompare(b.controller_session_id);
      if (controllerOrder !== 0) return controllerOrder;
      const childOrder = a.child_session_id.localeCompare(b.child_session_id);
      if (childOrder !== 0) return childOrder;
      return a.pairing_id.localeCompare(b.pairing_id);
    });
}

export function normalizeControllerPairingList(raw: unknown): ControllerPairing[] {
  if (!Array.isArray(raw)) {
    return [];
  }
  const map = new Map<string, ControllerPairing>();
  for (const entry of raw) {
    try {
      const pairing = normalizeControllerPairing(entry);
      map.set(`${pairing.controller_session_id}|${pairing.child_session_id}`, pairing);
    } catch (err) {
      console.warn('[api] skipping invalid controller pairing payload', err);
    }
  }
  return sortControllerPairings(Array.from(map.values()));
}

type ControllerPairingRequestBody = {
  child_session_id: string;
  prompt_template?: string | null;
  update_cadence: ControllerUpdateCadence;
};

export async function createControllerPairing(
  controllerSessionId: string,
  body: ControllerPairingRequestBody,
  token: string | null,
  baseUrl?: string,
): Promise<ControllerPairing> {
  const res = await fetch(`${base(baseUrl)}/sessions/${controllerSessionId}/controllers`, {
    method: 'POST',
    headers: authHeaders(token),
    body: JSON.stringify(body),
  });
  if (res.status === 404) {
    throw new Error('controller_pairing_api_unavailable');
  }
  if (!res.ok) {
    throw new Error(`createControllerPairing failed ${res.status}`);
  }
  const payload = await res.json();
  return normalizeControllerPairing(payload);
}

export async function listControllerPairingsForController(
  controllerSessionId: string,
  token: string | null,
  baseUrl?: string,
): Promise<ControllerPairing[]> {
  const res = await fetch(`${base(baseUrl)}/sessions/${controllerSessionId}/controllers`, {
    headers: authHeaders(token),
  });
  if (res.status === 404) {
    return [];
  }
  if (!res.ok) {
    throw new Error(`listControllerPairingsForController failed ${res.status}`);
  }
  const payload = await res.json();
  return normalizeControllerPairingList(payload);
}

export async function listControllerPairingsForControllers(
  controllerSessionIds: string[],
  token: string | null,
  baseUrl?: string,
): Promise<ControllerPairing[]> {
  if (controllerSessionIds.length === 0) {
    return [];
  }
  const results = await Promise.all(
    controllerSessionIds.map(async (sessionId) => {
      try {
        return await listControllerPairingsForController(sessionId, token, baseUrl);
      } catch (err) {
        console.error('[api] listControllerPairingsForController failed', {
          sessionId,
          error: err,
        });
        return [] as ControllerPairing[];
      }
    }),
  );
  const map = new Map<string, ControllerPairing>();
  for (const batch of results) {
    for (const pairing of batch) {
      map.set(pairing.pairing_id, pairing);
    }
  }
  return sortControllerPairings(Array.from(map.values()));
}

export async function deleteControllerPairing(
  controllerSessionId: string,
  childSessionId: string,
  token: string | null,
  baseUrl?: string,
): Promise<void> {
  const res = await fetch(`${base(baseUrl)}/sessions/${controllerSessionId}/controllers/${childSessionId}`, {
    method: 'DELETE',
    headers: authHeaders(token),
  });
  if (res.status === 404) {
    throw new Error('controller_pairing_api_unavailable');
  }
  if (!res.ok) {
    throw new Error(`deleteControllerPairing failed ${res.status}`);
  }
}

function cloneMetadata(metadata: unknown): Record<string, any> {
  if (metadata && typeof metadata === 'object' && !Array.isArray(metadata)) {
    return { ...(metadata as Record<string, any>) };
  }
  return {};
}

export function sessionRoleFromMetadata(metadata: unknown): SessionRole | null {
  if (!metadata || typeof metadata !== 'object') {
    return null;
  }
  const value = (metadata as any).role;
  if (value === 'agent' || value === 'application') {
    return value;
  }
  return null;
}

export function deriveSessionRole(
  session: SessionSummary,
  assignments?: ControllerPairing[],
): SessionRole {
  const metaRole = sessionRoleFromMetadata(session.metadata);
  if (metaRole) {
    return metaRole;
  }
  if (assignments?.some((pairing) => pairing.controller_session_id === session.session_id)) {
    return 'agent';
  }
  return 'application';
}

export function buildMetadataWithRole(metadata: unknown, role: SessionRole): Record<string, any> {
  const base = cloneMetadata(metadata);
  base.role = role;
  return base;
}

export async function updateSessionMetadata(
  sessionId: string,
  body: { metadata?: any; location_hint?: string | null },
  token: string | null,
  baseUrl?: string,
): Promise<void> {
  const res = await fetch(`${base(baseUrl)}/sessions/${sessionId}`, {
    method: 'PATCH',
    headers: authHeaders(token),
    body: JSON.stringify({
      metadata: body.metadata ?? null,
      location_hint: body.location_hint ?? null,
    }),
  });
  if (!res.ok) {
    throw new Error(`updateSessionMetadata failed ${res.status}`);
  }
}

export async function updateSessionRole(
  session: SessionSummary,
  role: SessionRole,
  token: string | null,
  baseUrl?: string,
): Promise<void> {
  const metadata = buildMetadataWithRole(session.metadata, role);
  await updateSessionMetadata(
    session.session_id,
    {
      metadata,
      location_hint: session.location_hint ?? null,
    },
    token,
    baseUrl,
  );
}

export async function updateSessionRoleById(
  sessionId: string,
  role: SessionRole,
  token: string | null,
  baseUrl?: string,
  metadata?: unknown,
  locationHint?: string | null,
): Promise<void> {
  const payload = buildMetadataWithRole(metadata, role);
  await updateSessionMetadata(
    sessionId,
    {
      metadata: payload,
      location_hint: locationHint ?? null,
    },
    token,
    baseUrl,
  );
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

// ---- Canvas Layout (v3) API ----

export type CanvasLayout = {
  version: 3;
  viewport: { zoom: number; pan: { x: number; y: number } };
  tiles: Record<
    string,
    {
      id: string;
      kind: 'application';
      position: { x: number; y: number };
      size: { width: number; height: number };
      zIndex: number;
      groupId?: string;
      zoom?: number;
      locked?: boolean;
      toolbarPinned?: boolean;
      metadata?: Record<string, any>;
    }
  >;
  agents: Record<
    string,
    {
      id: string;
      position: { x: number; y: number };
      size: { width: number; height: number };
      zIndex: number;
      icon?: string;
      status?: 'idle' | 'controlling';
    }
  >;
  groups: Record<
    string,
    {
      id: string;
      name?: string;
      memberIds: string[];
      position: { x: number; y: number };
      size: { width: number; height: number };
      zIndex: number;
      collapsed?: boolean;
      padding?: number;
    }
  >;
  controlAssignments: Record<string, { controllerId: string; targetType: 'tile' | 'group'; targetId: string }>;
  metadata: { createdAt: number; updatedAt: number; migratedFrom?: number };
};

export async function getCanvasLayout(id: string, token: string | null, baseUrl?: string): Promise<CanvasLayout> {
  const res = await fetch(`${base(baseUrl)}/private-beaches/${id}/layout`, {
    headers: authHeaders(token),
  });
  if (!res.ok) throw new Error(`getCanvasLayout failed ${res.status}`);
  const data = (await res.json()) as Partial<CanvasLayout>;
  if (data.version !== 3) {
    throw new Error('invalid canvas layout: version');
  }
  const tiles: CanvasLayout['tiles'] = {};
  for (const [tileId, raw] of Object.entries(data.tiles ?? {})) {
    tiles[tileId] = {
      kind: 'application',
      id: raw?.id ?? tileId,
      position: raw?.position ?? { x: 0, y: 0 },
      size: raw?.size ?? { width: 0, height: 0 },
      zIndex: raw?.zIndex ?? 1,
      groupId: raw?.groupId,
      zoom: raw?.zoom,
      locked: raw?.locked,
      toolbarPinned: raw?.toolbarPinned,
      metadata:
        raw && typeof raw === 'object' && 'metadata' in raw && raw?.metadata && typeof raw.metadata === 'object'
          ? { ...(raw.metadata as Record<string, any>) }
          : undefined,
    };
  }
  const groups: CanvasLayout['groups'] = {};
  for (const [groupId, raw] of Object.entries(data.groups ?? {})) {
    groups[groupId] = {
      id: raw?.id ?? groupId,
      name: raw?.name,
      memberIds: Array.isArray(raw?.memberIds) ? raw.memberIds : [],
      position: raw?.position ?? { x: 0, y: 0 },
      size: raw?.size ?? { width: 0, height: 0 },
      zIndex: raw?.zIndex ?? 1,
      collapsed: raw?.collapsed,
      padding: typeof raw?.padding === 'number' ? raw.padding : 16,
    };
  }
  return {
    version: 3,
    viewport: data.viewport ?? { zoom: 1, pan: { x: 0, y: 0 } },
    tiles,
    agents: data.agents ?? {},
    groups,
    controlAssignments: data.controlAssignments ?? {},
    metadata: data.metadata ?? { createdAt: Date.now(), updatedAt: Date.now() },
  };
}

export async function putCanvasLayout(
  id: string,
  layout: CanvasLayout,
  token: string | null,
  baseUrl?: string,
): Promise<CanvasLayout> {
  const res = await fetch(`${base(baseUrl)}/private-beaches/${id}/layout`, {
    method: 'PUT',
    headers: authHeaders(token),
    body: JSON.stringify(layout),
  });
  if (!res.ok) throw new Error(`putCanvasLayout failed ${res.status}`);
  const data = (await res.json()) as CanvasLayout;
  if (data.version !== 3) {
    throw new Error('invalid canvas layout: version');
  }
  const groups: CanvasLayout['groups'] = {};
  for (const [groupId, raw] of Object.entries(data.groups ?? {})) {
    groups[groupId] = {
      ...raw,
      id: raw?.id ?? groupId,
      memberIds: Array.isArray(raw?.memberIds) ? raw.memberIds : [],
      padding: typeof raw?.padding === 'number' ? raw.padding : 16,
    };
  }
  return { ...data, groups };
}

// ---- Batch Controller Assignment (manager) ----

export type ControllerAssignment = {
  controller_session_id: string;
  child_session_id: string;
  prompt_template?: string | null;
  update_cadence?: ControllerUpdateCadence;
};

export type ControllerAssignmentResult = {
  controller_session_id: string;
  child_session_id: string;
  ok: boolean;
  error?: string;
  pairing?: ControllerPairing;
};

export async function batchControllerAssignments(
  privateBeachId: string,
  assignments: ControllerAssignment[],
  token: string | null,
  baseUrl?: string,
): Promise<ControllerAssignmentResult[]> {
  if (!Array.isArray(assignments) || assignments.length === 0) {
    throw new Error('assignments array required');
  }
  const res = await fetch(`${base(baseUrl)}/private-beaches/${privateBeachId}/controller-assignments/batch`, {
    method: 'POST',
    headers: authHeaders(token),
    body: JSON.stringify({ assignments }),
  });
  if (!res.ok) throw new Error(`batchControllerAssignments failed ${res.status}`);
  const data = await res.json();
  const results = Array.isArray(data?.results) ? data.results : [];
  return results as ControllerAssignmentResult[];
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
