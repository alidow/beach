import type { FastifyInstance } from 'fastify';
import { decodeJwt } from 'jose';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { loadConfig, type BeachGateConfig } from './config.js';
import { buildServer } from './server.js';

const baseEnv = {
  CLERK_MOCK: '1',
  BEACH_GATE_PORT: '0',
  BEACH_GATE_HOST: '127.0.0.1',
};

describe('Beach Gate server', () => {
  let server: FastifyInstance;
  let config: BeachGateConfig;

  beforeEach(async () => {
    config = loadConfig({
      ...baseEnv,
      BEACH_GATE_DEFAULT_ENTITLEMENTS: 'rescue:fallback,session:group',
    });

    server = await buildServer({ config, logger: false });
  });

  it('exposes the signing key via JWKS', async () => {
    const response = await server.inject({
      method: 'GET',
      url: '/.well-known/jwks.json',
    });

    expect(response.statusCode).toBe(200);
    const body = response.json() as { keys: Array<Record<string, string>> };
    expect(Array.isArray(body.keys)).toBe(true);
    expect(body.keys).toHaveLength(1);

    const entry = body.keys[0];
    expect(entry.kty).toBe('EC');
    expect(entry.crv).toBe('P-256');
    expect(entry.kid).toBe(config.signingKey.kid);
    expect(entry.alg).toBe('ES256');
    expect(entry.use).toBe('sig');
    expect(typeof entry.x).toBe('string');
    expect(typeof entry.y).toBe('string');
  });

  it('issues tokens with scope and scp claims derived from entitlements', async () => {
    const session = await performDeviceFinish(server);
    const payload = decodeJwt(session.access_token) as {
      sub: string;
      entitlements: string[];
      scope: string;
      scp: string[];
    };

    expect(payload.sub).toBe('mock-user');
    expect(payload.entitlements).toEqual(['rescue:fallback', 'session:group']);
    expect(payload.scope).toBe('rescue:fallback session:group');
    expect(payload.scp).toEqual(['rescue:fallback', 'session:group']);
  });

  describe('TURN credentials endpoint', () => {
    it('returns 503 when TURN config is not available', async () => {
      const localConfig = loadConfig({
        ...baseEnv,
        BEACH_GATE_DEFAULT_ENTITLEMENTS: 'private-beach:turn,rescue:fallback',
      });
      const localServer = await buildServer({ config: localConfig, logger: false });
      try {
        const session = await performDeviceFinish(localServer);
        const response = await localServer.inject({
          method: 'POST',
          url: '/turn/credentials',
          headers: {
            authorization: `Bearer ${session.access_token}`,
          },
        });

        expect(response.statusCode).toBe(503);
        const body = response.json() as Record<string, unknown>;
        expect(body.error).toBe('turn_unavailable');
      } finally {
        await localServer.close();
      }
    });

    it('rejects callers without the required entitlement', async () => {
      const localConfig = loadConfig({
        ...baseEnv,
        BEACH_GATE_DEFAULT_ENTITLEMENTS: 'rescue:fallback',
        BEACH_GATE_TURN_SECRET: 'test-secret',
        BEACH_GATE_TURN_URLS: 'turn:turn.private-beach.test:3478',
        BEACH_GATE_TURN_REALM: 'turn.private-beach.test',
        BEACH_GATE_TURN_TTL: '90',
      });
      const localServer = await buildServer({ config: localConfig, logger: false });
      try {
        const session = await performDeviceFinish(localServer);
        const response = await localServer.inject({
          method: 'POST',
          url: '/turn/credentials',
          headers: {
            authorization: `Bearer ${session.access_token}`,
          },
        });

        expect(response.statusCode).toBe(403);
        const body = response.json() as Record<string, unknown>;
        expect(body.error).toBe('forbidden');
      } finally {
        await localServer.close();
      }
    });

    it('issues TURN credentials when the entitlement is present', async () => {
      const localConfig = loadConfig({
        ...baseEnv,
        BEACH_GATE_DEFAULT_ENTITLEMENTS: 'rescue:fallback,private-beach:turn',
        BEACH_GATE_TURN_SECRET: 'another-secret',
        BEACH_GATE_TURN_URLS: 'turns:turn.private-beach.test:5349,turn:turn.private-beach.test:3478',
        BEACH_GATE_TURN_REALM: 'turn.private-beach.test',
        BEACH_GATE_TURN_TTL: '120',
      });
      const localServer = await buildServer({ config: localConfig, logger: false });
      try {
        const session = await performDeviceFinish(localServer);
        const response = await localServer.inject({
          method: 'POST',
          url: '/turn/credentials',
          headers: {
            authorization: `Bearer ${session.access_token}`,
          },
        });

        expect(response.statusCode).toBe(200);
        const body = response.json() as Record<string, unknown>;

        expect(body.realm).toBe('turn.private-beach.test');
        expect(body.ttl_seconds).toBe(120);
        expect(typeof body.expires_at).toBe('number');
        expect(Array.isArray(body.iceServers)).toBe(true);

        const servers = body.iceServers as Array<Record<string, unknown>>;
        expect(servers.length).toBe(2);
        for (const serverInfo of servers) {
          expect(typeof serverInfo.urls).toBe('string');
          expect(typeof serverInfo.username).toBe('string');
          expect(typeof serverInfo.credential).toBe('string');
          expect(serverInfo.credentialType).toBe('password');
          expect((serverInfo.username as string).split(':')[2]).toBe('standard');
          expect(serverInfo.username as string).toContain('mock-user');
        }
      } finally {
        await localServer.close();
      }
    });
  });

  describe('viewer credential endpoint', () => {
    it('issues signed viewer tokens when configured', async () => {
      const viewerEnv = {
        ...baseEnv,
        BEACH_GATE_DEFAULT_ENTITLEMENTS: 'rescue:fallback',
        BEACH_GATE_VIEWER_TOKEN_SECRET: Buffer.from('viewer-secret').toString('base64'),
        BEACH_GATE_VIEWER_SERVICE_TOKENS: 'manager-token',
        BEACH_GATE_VIEWER_TOKEN_TTL: '30',
      };
      const localConfig = loadConfig(viewerEnv);
      const localServer = await buildServer({ config: localConfig, logger: false });
      try {
        // missing auth
        const unauth = await localServer.inject({
          method: 'POST',
          url: '/viewer/credentials',
        });
        expect(unauth.statusCode).toBe(401);

        const badBody = await localServer.inject({
          method: 'POST',
          url: '/viewer/credentials',
          headers: { authorization: 'Bearer manager-token' },
          payload: { sessionId: 'abc' },
        });
        expect(badBody.statusCode).toBe(400);

        const response = await localServer.inject({
          method: 'POST',
          url: '/viewer/credentials',
          headers: { authorization: 'Bearer manager-token' },
          payload: {
            sessionId: 'session-123',
            joinCode: 'ABC123',
            privateBeachId: 'pb-456',
          },
        });

        expect(response.statusCode).toBe(201);
        const body = response.json() as Record<string, unknown>;
        expect(typeof body.token).toBe('string');
        expect(typeof body.expires_at).toBe('number');
        expect(typeof body.expires_in).toBe('number');
      } finally {
        await localServer.close();
      }
    });
  });

  describe('auth exchange endpoint', () => {
    it('issues gate tokens when provided a valid Clerk token', async () => {
      const response = await server.inject({
        method: 'POST',
        url: '/auth/exchange',
        headers: {
          authorization: 'Bearer mock-clerk-token',
        },
      });

      expect(response.statusCode).toBe(201);
      const body = response.json() as Record<string, unknown>;
      expect(typeof body.access_token).toBe('string');
      expect(body.token_type).toBe('Bearer');
      expect(body.entitlements).toEqual(['rescue:fallback', 'session:group']);
    });

    it('rejects requests without a Clerk token', async () => {
      const response = await server.inject({
        method: 'POST',
        url: '/auth/exchange',
      });

      expect(response.statusCode).toBe(401);
      const body = response.json() as Record<string, unknown>;
      expect(body.error).toBe('unauthorized');
    });
  });

  afterEach(async () => {
    await server.close();
  });

  it('completes the device flow and returns tokens with entitlements', async () => {
    const startResponse = await server.inject({
      method: 'POST',
      url: '/device/start',
    });

    expect(startResponse.statusCode).toBe(200);
    const startBody = startResponse.json() as Record<string, unknown>;
    expect(typeof startBody.device_code).toBe('string');
    expect(typeof startBody.user_code).toBe('string');
    expect(typeof startBody.verification_uri).toBe('string');

    const finishResponse = await server.inject({
      method: 'POST',
      url: '/device/finish',
      payload: {
        deviceCode: startBody.device_code,
      },
    });

    expect(finishResponse.statusCode).toBe(201);
    const finishBody = finishResponse.json() as Record<string, unknown>;
    expect(typeof finishBody.access_token).toBe('string');
    expect(typeof finishBody.refresh_token).toBe('string');
    expect(Array.isArray(finishBody.entitlements)).toBe(true);
    expect(finishBody.entitlements).toContain('rescue:fallback');
    expect(typeof finishBody.profile).toBe('string');
    expect(typeof finishBody.tier).toBe('string');
  });

  it('rotates refresh tokens and issues new access tokens', async () => {
    const initial = await performDeviceFinish(server);

    const refreshResponse = await server.inject({
      method: 'POST',
      url: '/token/refresh',
      payload: {
        refreshToken: initial.refresh_token,
      },
    });

    expect(refreshResponse.statusCode).toBe(200);
    const refreshBody = refreshResponse.json() as Record<string, unknown>;
    expect(typeof refreshBody.access_token).toBe('string');
    expect(refreshBody.access_token).not.toBe(initial.access_token);
    expect(typeof refreshBody.refresh_token).toBe('string');
    expect(refreshBody.refresh_token).not.toBe(initial.refresh_token);
  });

  it('protects entitlements endpoint and returns details with a valid token', async () => {
    const initial = await performDeviceFinish(server);

    const unauthorized = await server.inject({
      method: 'GET',
      url: '/entitlements',
    });
    expect(unauthorized.statusCode).toBe(401);

    const authorized = await server.inject({
      method: 'GET',
      url: '/entitlements',
      headers: {
        authorization: `Bearer ${initial.access_token}`,
      },
    });

    expect(authorized.statusCode).toBe(200);
    const data = authorized.json() as Record<string, unknown>;
    expect(data.user_id).toBe('mock-user');
    expect(data.entitlements).toContain('rescue:fallback');
  });

  it('verifies access tokens and rejects invalid ones', async () => {
    const initial = await performDeviceFinish(server);

    const verifyValid = await server.inject({
      method: 'POST',
      url: '/authz/verify',
      payload: {
        token: initial.access_token,
      },
    });

    expect(verifyValid.statusCode).toBe(200);
    const validBody = verifyValid.json() as Record<string, unknown>;
    expect(validBody.valid).toBe(true);
    expect(validBody.user_id).toBe('mock-user');

    const verifyInvalid = await server.inject({
      method: 'POST',
      url: '/authz/verify',
      payload: {
        token: 'this-is-not-valid',
      },
    });

    expect(verifyInvalid.statusCode).toBe(401);
    const invalidBody = verifyInvalid.json() as Record<string, unknown>;
    expect(invalidBody.error).toBe('invalid_token');
  });
});

async function performDeviceFinish(server: FastifyInstance) {
  const startResponse = await server.inject({
    method: 'POST',
    url: '/device/start',
  });

  const startBody = startResponse.json() as Record<string, unknown>;

  const finishResponse = await server.inject({
    method: 'POST',
    url: '/device/finish',
    payload: {
      deviceCode: startBody.device_code,
    },
  });

  return finishResponse.json() as Record<string, string>;
}
