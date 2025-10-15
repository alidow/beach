import type { FastifyInstance } from 'fastify';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { loadConfig, type BeachGateConfig } from './config.js';
import { buildServer } from './server.js';

describe('Beach Gate server', () => {
  let server: FastifyInstance;
  let config: BeachGateConfig;

  beforeEach(async () => {
    config = loadConfig({
      CLERK_MOCK: '1',
      BEACH_GATE_PORT: '0',
      BEACH_GATE_HOST: '127.0.0.1',
      BEACH_GATE_DEFAULT_ENTITLEMENTS: 'rescue:fallback,session:group',
    });

    server = await buildServer({ config, logger: false });
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
