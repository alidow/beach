import { useRouter } from 'next/router';
import Head from 'next/head';
import ErrorPage from 'next/error';
import { useCallback, useEffect, useMemo, useState } from 'react';
import CanvasSurface from '../../components/CanvasSurface';
import SessionDrawer from '../../components/SessionDrawer';
import { Badge } from '../../components/ui/badge';
import {
  listSessions,
  getCanvasLayout,
  buildMetadataWithRole,
  type CanvasLayout,
  type SessionSummary,
  type SessionRole,
} from '../../lib/api';
import type { SessionCredentialOverride, TerminalViewerState } from '../../hooks/terminalViewerTypes';
import { createStaticTerminalViewer } from '../../sandbox/staticTerminal';
import { resolveTerminalFixture } from '../../sandbox/fixtures';

const SANDBOX_ENABLED =
  process.env.NEXT_PUBLIC_ENABLE_PRIVATE_BEACH_SANDBOX === 'true' || process.env.NODE_ENV !== 'production';
const DEFAULT_MANAGER_URL = process.env.NEXT_PUBLIC_MANAGER_URL || 'http://localhost:8080';
const DEFAULT_PRIVATE_BEACH_ID = 'sandbox';
const DEFAULT_TILE_WIDTH = 448;
const DEFAULT_TILE_HEIGHT = 448;
const DEFAULT_TILE_GAP = 32;

type SessionSpec = {
  id: string;
  role: SessionRole;
  title?: string;
  passcode?: string;
};

type SandboxConfig = {
  ready: boolean;
  privateBeachId: string | null;
  managerUrl: string;
  managerToken: string | null;
  viewerToken: string | null;
  sessionSpecs: SessionSpec[];
  specById: Map<string, SessionSpec>;
  passcodeEntries: Array<[string, string]>;
  passcodeMap: Map<string, string>;
  titleEntries: Array<[string, string]>;
  titleMap: Map<string, string>;
  terminalFixtureEntries: Array<[string, string]>;
  terminalFixtureMap: Map<string, string>;
  shouldFetchFromApi: boolean;
  skipApi: boolean;
  signature: string;
};

function firstQueryValue(value: string | string[] | undefined): string | null {
  if (Array.isArray(value)) {
    return value.find((entry) => typeof entry === 'string' && entry.trim().length > 0)?.trim() ?? null;
  }
  if (typeof value === 'string' && value.trim().length > 0) {
    return value.trim();
  }
  return null;
}

function splitList(value: string | string[] | undefined): string[] {
  const result: string[] = [];
  if (!value) return result;
  const list = Array.isArray(value) ? value : [value];
  for (const raw of list) {
    for (const part of raw.split(',')) {
      const trimmed = part.trim();
      if (trimmed.length > 0) {
        result.push(trimmed);
      }
    }
  }
  return result;
}

function decodeParam(value: string | null): string | null {
  if (!value) return null;
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function parseSessionEntries(input: string | string[] | undefined, role: SessionRole): SessionSpec[] {
  return splitList(input)
    .map((entry) => {
      const parts = entry.split('|').map((part) => decodeParam(part.trim()) ?? '');
      if (!parts[0]) {
        return null;
      }
      const [id, titlePart, passcodePart] = parts;
      const title = titlePart && titlePart.length > 0 ? titlePart : undefined;
      const passcode = passcodePart && passcodePart.length > 0 ? passcodePart : undefined;
      return {
        id,
        role,
        title,
        passcode,
      } satisfies SessionSpec;
    })
    .filter((spec): spec is SessionSpec => Boolean(spec));
}

function parseGeneralSessions(input: string | string[] | undefined): SessionSpec[] {
  return splitList(input)
    .map((entry) => {
      const parts = entry.split('|').map((part) => decodeParam(part.trim()) ?? '');
      if (!parts[0]) {
        return null;
      }
      const id = parts[0];
      let index = 1;
      let role: SessionRole = 'application';
      if (parts[index]) {
        const normalized = parts[index].toLowerCase();
        if (normalized === 'agent' || normalized === 'application') {
          role = normalized as SessionRole;
          index += 1;
        }
      }
      let title: string | undefined;
      if (parts[index] && parts[index].length > 0) {
        title = parts[index];
        index += 1;
      }
      let passcode: string | undefined;
      if (parts[index] && parts[index].length > 0) {
        passcode = parts[index];
      }
      return {
        id,
        role,
        title,
        passcode,
      } satisfies SessionSpec;
    })
    .filter((spec): spec is SessionSpec => Boolean(spec));
}

function parseKeyValueList(value: string | string[] | undefined): Map<string, string> {
  const entries = new Map<string, string>();
  for (const raw of splitList(value)) {
    const [keyPart, ...rest] = raw.split(':');
    const key = decodeParam(keyPart.trim());
    if (!key) continue;
    const joined = rest.join(':');
    const decodedValue = decodeParam(joined.trim()) ?? '';
    if (decodedValue.length > 0) {
      entries.set(key, decodedValue);
    }
  }
  return entries;
}

function mergeSpec(existing: SessionSpec | undefined, incoming: SessionSpec): SessionSpec {
  if (!existing) {
    return incoming;
  }
  return {
    id: incoming.id,
    role: incoming.role ?? existing.role,
    title: incoming.title ?? existing.title,
    passcode: incoming.passcode ?? existing.passcode,
  };
}

function parseSandboxConfig(query: ReturnType<typeof useRouter>['query'], isReady: boolean): SandboxConfig {
  const privateBeachId = decodeParam(firstQueryValue(query.privateBeachId ?? query.pb));
  const managerUrl =
    decodeParam(firstQueryValue(query.managerUrl ?? query.manager)) ?? DEFAULT_MANAGER_URL;
  const managerToken = decodeParam(firstQueryValue(query.managerToken ?? query.token));
  const viewerToken =
    decodeParam(firstQueryValue(query.viewerToken)) ?? managerToken ?? null;
  const skipApiRaw = firstQueryValue(query.skipApi ?? query.skip_api);
  const skipApi = Boolean(skipApiRaw && ['1', 'true', 'yes'].includes(skipApiRaw.toLowerCase()));
  const passcodeMap = parseKeyValueList(query.passcodes ?? query.passcode);
  const titleMap = parseKeyValueList(query.titles ?? query.title);
  const fixtureMap = parseKeyValueList(
    query.terminalFixtures ?? query.terminalFixture ?? query.mockTerminals ?? query.fixtures,
  );
  const specById = new Map<string, SessionSpec>();

  const upsert = (spec: SessionSpec) => {
    const existing = specById.get(spec.id);
    specById.set(spec.id, mergeSpec(existing, spec));
  };

  parseGeneralSessions(query.sessions ?? query.session).forEach(upsert);
  for (const [id, passcode] of passcodeMap.entries()) {
    upsert({ id, role: 'application', passcode });
  }
  for (const [id, title] of titleMap.entries()) {
    upsert({ id, role: 'application', title });
  }
  parseSessionEntries(query.applications ?? query.apps, 'application').forEach(upsert);
  parseSessionEntries(query.agents, 'agent').forEach(upsert);
  for (const sessionId of fixtureMap.keys()) {
    if (!specById.has(sessionId)) {
      upsert({ id: sessionId, role: 'application' });
    }
  }

  const sessionSpecs = Array.from(specById.values());
  const shouldFetchFromApi = Boolean(privateBeachId && managerToken && !skipApi);

  const signature = JSON.stringify({
    ready: isReady,
    privateBeachId,
    managerUrl,
    managerToken: managerToken ? 'present' : 'missing',
    viewerToken: viewerToken ? 'present' : 'missing',
    skipApi,
    specs: sessionSpecs.map((spec) => [spec.id, spec.role, spec.title ?? '', spec.passcode ?? '']),
    passcodes: Array.from(passcodeMap.entries()),
    titles: Array.from(titleMap.entries()),
    fixtures: Array.from(fixtureMap.entries()),
  });

  return {
    ready: isReady,
    privateBeachId,
    managerUrl,
    managerToken,
    viewerToken,
    sessionSpecs,
    specById,
    passcodeEntries: Array.from(passcodeMap.entries()),
    passcodeMap,
    titleEntries: Array.from(titleMap.entries()),
    titleMap,
    terminalFixtureEntries: Array.from(fixtureMap.entries()),
    terminalFixtureMap: fixtureMap,
    shouldFetchFromApi,
    skipApi,
    signature,
  };
}

function clonePlainMetadata(metadata: unknown): Record<string, any> {
  if (metadata && typeof metadata === 'object' && !Array.isArray(metadata)) {
    return { ...(metadata as Record<string, any>) };
  }
  return {};
}

function createStubSession(spec: SessionSpec, privateBeachId: string | null): SessionSummary {
  const metadata = buildMetadataWithRole({}, spec.role);
  if (spec.title) {
    metadata.title = spec.title;
  }
  return {
    session_id: spec.id,
    private_beach_id: privateBeachId ?? DEFAULT_PRIVATE_BEACH_ID,
    harness_type: 'sandbox.stub',
    capabilities: [],
    metadata,
    version: 'sandbox',
    harness_id: 'sandbox',
    controller_token: null,
    controller_expires_at_ms: null,
    pending_actions: 0,
    pending_unacked: 0,
    last_health: null,
    location_hint: null,
  };
}

function emptyCanvasLayout(): CanvasLayout {
  const now = Date.now();
  return {
    version: 3,
    viewport: { zoom: 1, pan: { x: 0, y: 0 } },
    tiles: {},
    agents: {},
    groups: {},
    controlAssignments: {},
    metadata: { createdAt: now, updatedAt: now },
  };
}

function ensureLayoutBase(layout: CanvasLayout | null): CanvasLayout {
  if (!layout) {
    return emptyCanvasLayout();
  }
  const now = Date.now();
  const tiles: CanvasLayout['tiles'] = {};
  for (const [tileId, tile] of Object.entries(layout.tiles ?? {})) {
    tiles[tileId] = {
      id: tile.id ?? tileId,
      kind: tile.kind ?? 'application',
      position: tile.position ?? { x: 0, y: 0 },
      size: tile.size ?? { width: DEFAULT_TILE_WIDTH, height: DEFAULT_TILE_HEIGHT },
      zIndex: tile.zIndex ?? 1,
      groupId: tile.groupId,
      zoom: tile.zoom ?? 1,
      locked: tile.locked ?? false,
      toolbarPinned: tile.toolbarPinned ?? false,
      metadata: tile.metadata ?? {},
    };
  }
  return {
    version: 3,
    viewport: layout.viewport ?? { zoom: 1, pan: { x: 0, y: 0 } },
    tiles,
    agents: layout.agents ?? {},
    groups: layout.groups ?? {},
    controlAssignments: layout.controlAssignments ?? {},
    metadata: {
      createdAt: layout.metadata?.createdAt ?? now,
      updatedAt: layout.metadata?.updatedAt ?? now,
      migratedFrom: layout.metadata?.migratedFrom,
    },
  };
}

function withUpdatedTimestamp(layout: CanvasLayout): CanvasLayout {
  const now = Date.now();
  return {
    ...layout,
    metadata: {
      ...(layout.metadata ?? { createdAt: now, updatedAt: now }),
      updatedAt: now,
    },
  };
}

function computeAutoPosition(index: number) {
  const column = index % 4;
  const row = Math.floor(index / 4);
  return {
    x: column * (DEFAULT_TILE_WIDTH + DEFAULT_TILE_GAP),
    y: row * (DEFAULT_TILE_HEIGHT + DEFAULT_TILE_GAP),
  };
}

function ensureLayoutForSessions(layout: CanvasLayout | null, sessionIds: string[]): CanvasLayout {
  const base = ensureLayoutBase(layout);
  const tiles = { ...base.tiles };
  let changed = false;
  let index = Object.keys(tiles).length;
  for (const sessionId of sessionIds) {
    if (tiles[sessionId]) {
      continue;
    }
    const position = computeAutoPosition(index);
    index += 1;
    tiles[sessionId] = {
      id: sessionId,
      kind: 'application',
      position,
      size: { width: DEFAULT_TILE_WIDTH, height: DEFAULT_TILE_HEIGHT },
      zIndex: index,
      zoom: 1,
      locked: false,
      toolbarPinned: false,
      metadata: {},
    };
    changed = true;
  }
  for (const existingId of Object.keys(tiles)) {
    if (!sessionIds.includes(existingId)) {
      delete tiles[existingId];
      changed = true;
    }
  }
  if (!changed) {
    return base;
  }
  return withUpdatedTimestamp({
    ...base,
    tiles,
  });
}

function resolveSessions(config: SandboxConfig, fetched: SessionSummary[]): SessionSummary[] {
  const result = new Map<string, SessionSummary>();
  for (const session of fetched) {
    const spec = config.specById.get(session.session_id) ?? null;
    let metadata = spec
      ? buildMetadataWithRole(session.metadata, spec.role)
      : clonePlainMetadata(session.metadata);
    if (spec?.title) {
      metadata.title = spec.title;
    } else {
      const titleOverride = config.titleMap.get(session.session_id);
      if (titleOverride) {
        metadata.title = titleOverride;
      }
    }
    result.set(session.session_id, {
      ...session,
      metadata,
    });
  }
  for (const spec of config.sessionSpecs) {
    if (result.has(spec.id)) {
      continue;
    }
    result.set(spec.id, createStubSession(spec, config.privateBeachId));
  }
  return Array.from(result.values()).sort((a, b) => a.session_id.localeCompare(b.session_id));
}

function buildViewerOverrides(
  passcodeEntries: Array<[string, string]>,
  sessionSpecs: SessionSpec[],
  managerToken: string | null,
): Record<string, SessionCredentialOverride> {
  const overrides: Record<string, SessionCredentialOverride> = {};
  const authorizationToken = managerToken ?? undefined;
  for (const [sessionId, passcode] of passcodeEntries) {
    overrides[sessionId] = {
      passcode,
      authorizationToken,
      skipCredentialFetch: true,
    };
  }
  for (const spec of sessionSpecs) {
    if (spec.passcode && spec.passcode.length > 0) {
      overrides[spec.id] = {
        passcode: spec.passcode,
        authorizationToken,
        skipCredentialFetch: true,
      };
    }
  }
  return overrides;
}

function sessionRole(session: SessionSummary): SessionRole {
  const metadata = session.metadata;
  if (metadata && typeof metadata === 'object' && !Array.isArray(metadata)) {
    const value = (metadata as any).role;
    if (value === 'agent' || value === 'application') {
      return value;
    }
  }
  return 'application';
}

function redactToken(token: string | null): string {
  if (!token) return '—';
  if (token.length <= 8) return token;
  return `${token.slice(0, 4)}…${token.slice(-4)}`;
}

function PrivateBeachSandboxPage() {
  const router = useRouter();
  const config = useMemo(() => parseSandboxConfig(router.query, router.isReady), [router.isReady, router.query]);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [layout, setLayout] = useState<CanvasLayout | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selectedSession, setSelectedSession] = useState<SessionSummary | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);

  const viewerToken = config.viewerToken ?? config.managerToken ?? null;
  const viewerOverrides = useMemo(
    () => buildViewerOverrides(config.passcodeEntries, config.sessionSpecs, config.managerToken),
    [config.passcodeEntries, config.sessionSpecs, config.managerToken],
  );
  const viewerStateOverrides = useMemo(() => {
    const overrides: Record<string, TerminalViewerState> = {};
    for (const [sessionId, fixtureKey] of config.terminalFixtureEntries) {
      const fixture = resolveTerminalFixture(fixtureKey);
      if (!fixture) {
        console.warn('[sandbox-debug] missing terminal fixture', { sessionId, fixtureKey });
        continue;
      }
      console.info('[sandbox-debug] apply terminal fixture', {
        sessionId,
        fixtureKey,
      });
      overrides[sessionId] = createStaticTerminalViewer(fixture, { viewportRows: 24 });
    }
    return overrides;
  }, [config.terminalFixtureEntries]);

  const partitionedSessions = useMemo(() => {
    const agents: SessionSummary[] = [];
    const applications: SessionSummary[] = [];
    for (const session of sessions) {
      if (sessionRole(session) === 'agent') {
        agents.push(session);
      } else {
        applications.push(session);
      }
    }
    return { agents, applications };
  }, [sessions]);

  useEffect(() => {
    if (!router.isReady || !SANDBOX_ENABLED) {
      return;
    }
    let cancelled = false;
    (async () => {
      setLoading(true);
      setError(null);
      try {
        let fetchedSessions: SessionSummary[] = [];
        let fetchedLayout: CanvasLayout | null = null;
        if (config.shouldFetchFromApi && config.privateBeachId && config.managerToken) {
          try {
            fetchedSessions = await listSessions(
              config.privateBeachId,
              config.managerToken,
              config.managerUrl,
            );
          } catch (sessionErr) {
            console.error('[sandbox] failed to fetch sessions', sessionErr);
            setError((prev) => prev ?? 'Failed to load sessions from manager API.');
          }
          try {
            fetchedLayout = await getCanvasLayout(
              config.privateBeachId,
              config.managerToken,
              config.managerUrl,
            );
          } catch (layoutErr) {
            console.warn('[sandbox] failed to fetch canvas layout', layoutErr);
          }
        }
        const resolved = resolveSessions(config, fetchedSessions);
        if (cancelled) {
          return;
        }
        setSessions(resolved);
        setLayout((prev) =>
          ensureLayoutForSessions(
            fetchedLayout ?? prev,
            resolved
              .filter((session) => sessionRole(session) === 'application')
              .map((session) => session.session_id),
          ),
        );
      } catch (err) {
        if (!cancelled) {
          const message = err instanceof Error ? err.message : String(err);
          setError(message);
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [router.isReady, config.signature]);

  useEffect(() => {
    setLayout((prev) =>
      ensureLayoutForSessions(
        prev,
        partitionedSessions.applications.map((session) => session.session_id),
      ),
    );
  }, [partitionedSessions.applications]);

  const handleSelect = useCallback((session: SessionSummary) => {
    setSelectedSession(session);
    setDrawerOpen(true);
  }, []);

  const handleRemove = useCallback((sessionId: string) => {
    setLayout((prev) => {
      if (!prev) return prev;
      const base = ensureLayoutBase(prev);
      if (!base.tiles[sessionId]) {
        return base;
      }
      const { [sessionId]: _omit, ...rest } = base.tiles;
      return withUpdatedTimestamp({
        ...base,
        tiles: rest,
      });
    });
  }, []);

  const handleLayoutChange = useCallback((next: CanvasLayout) => {
    setLayout(ensureLayoutBase(next));
  }, []);

  const handlePersistLayout = useCallback((next: CanvasLayout) => {
    setLayout(ensureLayoutBase(next));
  }, []);

  if (!SANDBOX_ENABLED) {
    return <ErrorPage statusCode={404} title="Not Found" />;
  }

  return (
    <>
      <Head>
        <title>Private Beach Sandbox</title>
      </Head>
      <div className="min-h-screen bg-background text-foreground">
        <div className="border-b border-border bg-muted/30 px-4 py-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <h1 className="text-lg font-semibold">Private Beach Dashboard Sandbox</h1>
              <p className="text-sm text-muted-foreground">
                Configure sessions via query parameters to exercise the dashboard without authentication.
              </p>
            </div>
            <div className="flex items-center gap-2">
              {loading ? <Badge variant="secondary">Loading</Badge> : <Badge variant="outline">Ready</Badge>}
              {config.skipApi && <Badge variant="secondary">API Disabled</Badge>}
            </div>
          </div>
        </div>
        <main className="flex flex-col gap-4 p-4">
          <section className="grid gap-3 rounded-lg border border-border bg-card p-4 text-sm shadow-sm">
            <div className="grid gap-1">
              <span className="font-medium text-muted-foreground">Manager URL</span>
              <span className="font-mono text-xs">{config.managerUrl}</span>
            </div>
            <div className="grid gap-1">
              <span className="font-medium text-muted-foreground">Private Beach ID</span>
              <span className="font-mono text-xs">{config.privateBeachId ?? '(not specified)'}</span>
            </div>
            <div className="grid gap-1">
              <span className="font-medium text-muted-foreground">Manager Token</span>
              <span className="font-mono text-xs">{redactToken(config.managerToken)}</span>
            </div>
            <div className="grid gap-1">
              <span className="font-medium text-muted-foreground">Viewer Token</span>
              <span className="font-mono text-xs">{redactToken(viewerToken)}</span>
            </div>
            <div className="grid gap-1">
              <span className="font-medium text-muted-foreground">Configured Sessions</span>
              <div className="flex flex-wrap gap-2">
                {sessions.length === 0 ? (
                  <span className="text-xs text-muted-foreground">No sessions defined.</span>
                ) : (
                  sessions.map((session) => {
                    const passcode = viewerOverrides[session.session_id]?.passcode ?? null;
                    const role = sessionRole(session);
                    const title =
                      (session.metadata && typeof session.metadata === 'object'
                        ? (session.metadata as any).title
                        : null) ?? session.session_id.slice(0, 8);
                    return (
                      <span
                        key={session.session_id}
                        className="rounded border border-border bg-background/60 px-2 py-1 text-xs"
                      >
                        <span className="font-semibold">{title}</span>{' '}
                        <span className="text-muted-foreground">({role})</span>
                        {passcode ? (
                          <span className="ml-2 font-mono text-[11px] text-muted-foreground">
                            code: {passcode}
                          </span>
                        ) : null}
                      </span>
                    );
                  })
                )}
              </div>
            </div>
          </section>
          {error && (
            <div className="rounded border border-red-500/40 bg-red-500/10 p-3 text-sm text-red-600 dark:text-red-400">
              {error}
            </div>
          )}
          <section className="min-h-[520px] rounded-lg border border-border bg-card/50 p-2">
            <CanvasSurface
              tiles={partitionedSessions.applications}
              agents={partitionedSessions.agents}
              layout={layout}
              onLayoutChange={handleLayoutChange}
              onPersistLayout={handlePersistLayout}
              onRemove={handleRemove}
              onSelect={handleSelect}
              privateBeachId={config.privateBeachId}
              managerToken={config.managerToken}
              managerUrl={config.managerUrl}
              viewerToken={viewerToken}
              viewerOverrides={viewerOverrides}
              viewerStateOverrides={viewerStateOverrides}
            />
          </section>
        </main>
        <SessionDrawer
          open={drawerOpen}
          onOpenChange={setDrawerOpen}
          session={drawerOpen ? selectedSession : null}
          managerUrl={config.managerUrl}
          token={config.managerToken}
        />
      </div>
    </>
  );
}

export default PrivateBeachSandboxPage;
