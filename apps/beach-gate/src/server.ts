import Fastify, { FastifyInstance, FastifyReply, FastifyRequest, FastifyServerOptions } from 'fastify';
import { BeachGateConfig } from './config.js';
import { ClerkClient, createClerkClient } from './clerk.js';
import { EntitlementStore } from './entitlements.js';
import { RefreshTokenStore } from './refresh-store.js';
import { TokenService } from './token-service.js';

interface DeviceStartBody {
  scope?: string;
  audience?: string;
}

interface DeviceFinishBody {
  deviceCode?: string;
}

interface TokenRefreshBody {
  refreshToken?: string;
}

interface VerifyBody {
  token?: string;
}

interface ServerDependencies {
  config: BeachGateConfig;
  clerk?: ClerkClient;
  entitlements?: EntitlementStore;
  refreshTokens?: RefreshTokenStore;
  tokens?: TokenService;
  logger?: FastifyServerOptions['logger'];
}

export async function buildServer(deps: ServerDependencies): Promise<FastifyInstance> {
  const { config } = deps;
  const fastify = Fastify({
    logger: deps.logger ?? true,
  });

  const clerk = deps.clerk ?? createClerkClient(config);
  const entitlements = deps.entitlements ?? new EntitlementStore(config);
  const refreshTokens = deps.refreshTokens ?? new RefreshTokenStore(config.refreshTokenTtlSeconds);
  const tokens = deps.tokens ?? new TokenService(config);

  fastify.get('/healthz', async () => ({ status: 'ok' }));

  fastify.post('/device/start', async (request, reply) => {
    const body = request.body as DeviceStartBody | undefined;

    try {
      const response = await clerk.startDeviceAuthorization({
        scope: body?.scope,
        audience: body?.audience,
        userAgent: request.headers['user-agent'],
      });

      return {
        device_code: response.deviceCode,
        user_code: response.userCode,
        verification_uri: response.verificationUri,
        verification_uri_complete: response.verificationUriComplete,
        expires_in: response.expiresIn,
        interval: response.interval,
      };
    } catch (error) {
      request.log.error(error, 'device.start_failed');
      return reply.status(502).send({ error: 'device_start_failed', detail: (error as Error).message });
    }
  });

  fastify.post('/device/finish', async (request, reply) => {
    const body = request.body as DeviceFinishBody | undefined;
    if (!body?.deviceCode) {
      return reply.status(400).send({ error: 'invalid_request', detail: 'deviceCode is required.' });
    }

    try {
      const clerkResult = await clerk.finishDeviceAuthorization(body.deviceCode);
      const ent = entitlements.resolve({
        userId: clerkResult.userId,
        email: clerkResult.email,
      });

      const refresh = refreshTokens.create({
        userId: clerkResult.userId,
        email: clerkResult.email,
        clerkRefreshToken: clerkResult.clerkRefreshToken,
        entitlements: ent.entitlements,
        tier: ent.tier,
        profile: ent.profile,
      });

      const access = await tokens.issueAccessToken({
        userId: clerkResult.userId,
        email: clerkResult.email,
        entitlements: ent.entitlements,
        tier: ent.tier,
        profile: ent.profile,
      });

      return reply.status(201).send({
        access_token: access.token,
        access_token_expires_in: access.expiresIn,
        refresh_token: refresh.token,
        refresh_token_expires_in: config.refreshTokenTtlSeconds,
        entitlements: ent.entitlements,
        tier: ent.tier,
        profile: ent.profile,
      });
    } catch (error) {
      request.log.error(error, 'device.finish_failed');
      return reply.status(502).send({ error: 'device_finish_failed', detail: (error as Error).message });
    }
  });

  fastify.post('/token/refresh', async (request, reply) => {
    const body = request.body as TokenRefreshBody | undefined;
    if (!body?.refreshToken) {
      return reply.status(400).send({ error: 'invalid_request', detail: 'refreshToken is required.' });
    }

    const record = refreshTokens.verify(body.refreshToken);
    if (!record) {
      return reply.status(401).send({ error: 'invalid_refresh_token' });
    }

    const ent = {
      entitlements: record.entitlements,
      tier: record.tier,
      profile: record.profile,
    };

    const rotated = refreshTokens.rotate(body.refreshToken, record);
    const access = await tokens.issueAccessToken({
      userId: record.userId,
      email: record.email,
      entitlements: ent.entitlements,
      tier: ent.tier,
      profile: ent.profile,
    });

    return {
      access_token: access.token,
      access_token_expires_in: access.expiresIn,
      refresh_token: rotated.token,
      refresh_token_expires_in: config.refreshTokenTtlSeconds,
      entitlements: ent.entitlements,
      tier: ent.tier,
      profile: ent.profile,
    };
  });

  fastify.get('/entitlements', { preHandler: authenticate(tokens) }, async (request) => {
    const token = request.accessToken!;
    return {
      user_id: token.sub,
      email: token.email,
      entitlements: token.entitlements,
      tier: token.tier,
      profile: token.profile,
      expires_at: token.exp ? token.exp * 1000 : undefined,
    };
  });

  fastify.post('/authz/verify', async (request, reply) => {
    const headerToken = extractBearerToken(request);
    const body = request.body as VerifyBody | undefined;
    const token = headerToken ?? body?.token;

    if (!token) {
      return reply.status(400).send({ error: 'invalid_request', detail: 'Provide token in Authorization header or body.token.' });
    }

    try {
      const verified = await tokens.verifyAccessToken(token);
      return {
        valid: true,
        user_id: verified.sub,
        email: verified.email,
        entitlements: verified.entitlements,
        tier: verified.tier,
        profile: verified.profile,
        expires_at: verified.exp ? verified.exp * 1000 : undefined,
      };
    } catch (error) {
      request.log.warn(error, 'token.verify_failed');
      return reply.status(401).send({ error: 'invalid_token', detail: (error as Error).message });
    }
  });

  return fastify;
}

function authenticate(tokens: TokenService) {
  return async (request: FastifyRequest, reply: FastifyReply): Promise<void> => {
    const token = extractBearerToken(request);
    if (!token) {
      await reply.status(401).send({ error: 'unauthorized', detail: 'Missing bearer token.' });
      return;
    }

    try {
      const verified = await tokens.verifyAccessToken(token);
      request.accessToken = verified;
    } catch (error) {
      request.log.warn(error, 'auth.invalid_token');
      await reply.status(401).send({ error: 'invalid_token', detail: (error as Error).message });
    }
  };
}

function extractBearerToken(request: FastifyRequest): string | undefined {
  const header = request.headers.authorization;
  if (!header) {
    return undefined;
  }

  const [scheme, token] = header.split(/\s+/);
  if (scheme?.toLowerCase() !== 'bearer' || !token) {
    return undefined;
  }
  return token;
}
