import { createHmac, type JsonWebKey } from 'node:crypto';
import Fastify, { FastifyInstance, FastifyReply, FastifyRequest, FastifyServerOptions } from 'fastify';
import { BeachGateConfig, TurnConfig, type SigningKeyMaterial, persistSigningKid } from './config.js';
import { ClerkClient, createClerkClient } from './clerk.js';
import { EntitlementStore } from './entitlements.js';
import { RefreshTokenStore } from './refresh-store.js';
import { TokenService, VerifiedAccessToken } from './token-service.js';

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

interface ViewerCredentialBody {
  sessionId?: string;
  joinCode?: string;
  privateBeachId?: string;
  ttlSeconds?: number;
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
  const jwksResponse = buildJwksResponse(config.signingKey);
  fastify.log.info({ kid: config.signingKey.kid }, 'beach-gate signing key ready');
  persistSigningKid(config.signingKey.kid, config.signingKidPath);

  fastify.get('/healthz', async () => ({ status: 'ok' }));
  fastify.get('/signing-key', async (_request, reply) => {
    reply.header('cache-control', 'public, max-age=60');
    return { kid: config.signingKey.kid };
  });
  fastify.get('/.well-known/jwks.json', async (_request, reply) => {
    reply.header('cache-control', 'public, max-age=60');
    return jwksResponse;
  });

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

  fastify.post('/turn/credentials', { preHandler: authenticate(tokens) }, async (request, reply) => {
    if (!config.turn) {
      request.log.warn('turn.credentials_unavailable');
      return reply.status(503).send({ error: 'turn_unavailable' });
    }

    const token = request.accessToken!;
    // In dev mode (mockClerk), allow all authenticated users to access TURN
    if (!config.mockClerk && !hasRequiredEntitlement(token.entitlements, config.turn.requiredEntitlements)) {
      request.log.warn({ userId: token.sub }, 'turn.credentials_forbidden');
      return reply.status(403).send({ error: 'forbidden', detail: 'TURN access not granted.' });
    }

    const issued = issueTurnCredentials(token, config.turn);
    return {
      realm: config.turn.realm,
      ttl_seconds: config.turn.ttlSeconds,
      expires_at: issued.expiresAt,
      iceServers: config.turn.urls.map((url) => ({
        urls: url,
        username: issued.username,
        credential: issued.credential,
        credentialType: 'password',
      })),
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

  fastify.post('/auth/exchange', async (request, reply) => {
    const clerkToken = extractBearerToken(request);
    if (!clerkToken) {
      return reply.status(401).send({ error: 'unauthorized', detail: 'Missing Clerk token.' });
    }

    try {
      const userInfo = await clerk.getUserInfo(clerkToken);
      if (!userInfo?.sub) {
        throw new Error('clerk_user_missing_sub');
      }

      const ent = entitlements.resolve({
        userId: userInfo.sub,
        email: typeof userInfo.email === 'string' ? userInfo.email : undefined,
      });

      const access = await tokens.issueAccessToken({
        userId: userInfo.sub,
        email: typeof userInfo.email === 'string' ? userInfo.email : undefined,
        entitlements: ent.entitlements,
        tier: ent.tier,
        profile: ent.profile,
      });

      return reply.status(201).send({
        access_token: access.token,
        expires_in: access.expiresIn,
        token_type: 'Bearer',
        entitlements: ent.entitlements,
        tier: ent.tier,
        profile: ent.profile,
      });
    } catch (error) {
      request.log.warn(error, 'auth.exchange_failed');
      return reply.status(401).send({ error: 'invalid_clerk_token' });
    }
  });

  if (config.viewerToken) {
    fastify.post('/viewer/credentials', async (request, reply) => {
      const bearer = extractBearerToken(request);
      if (!bearer || !config.viewerToken!.serviceTokens.includes(bearer)) {
        return reply.status(401).send({ error: 'unauthorized', detail: 'Invalid or missing bearer token.' });
      }

      const body = request.body as ViewerCredentialBody | undefined;
      const sessionId = body?.sessionId?.trim();
      const joinCode = body?.joinCode?.trim();

      if (!sessionId || !joinCode) {
        return reply
          .status(400)
          .send({ error: 'invalid_request', detail: 'sessionId and joinCode are required.' });
      }

      try {
        const issued = await tokens.issueViewerToken({
          sessionId,
          joinCode,
          privateBeachId: body?.privateBeachId?.trim(),
          ttlSeconds: body?.ttlSeconds,
        });
        return reply.status(201).send({
          token: issued.token,
          expires_at: issued.expiresAt,
          expires_in: issued.expiresIn,
        });
      } catch (error) {
        request.log.error(error, 'viewer.credentials_issue_failed');
        return reply.status(500).send({ error: 'viewer_token_failed', detail: (error as Error).message });
      }
    });
  } else {
    fastify.post('/viewer/credentials', async (_request, reply) => {
      return reply.status(503).send({ error: 'viewer_tokens_unavailable' });
    });
  }

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

function hasRequiredEntitlement(entitlements: string[], required: string[]): boolean {
  if (required.length === 0) {
    return true;
  }

  const normalized = new Set(entitlements.map((value) => value.toLowerCase()));
  return required.some((value) => normalized.has(value.toLowerCase()));
}

function issueTurnCredentials(token: VerifiedAccessToken, turn: TurnConfig) {
  const nowSeconds = Math.floor(Date.now() / 1000);
  const expiresAtSeconds = nowSeconds + turn.ttlSeconds;
  const username = `${expiresAtSeconds}:${token.sub}:${token.tier}`;
  const credential = createHmac('sha1', turn.secret).update(username).digest('base64');

  return {
    username,
    credential,
    expiresAt: expiresAtSeconds * 1000,
  };
}

interface EcJwk {
  kty: 'EC';
  crv: 'P-256';
  x: string;
  y: string;
  kid: string;
  alg: 'ES256';
  use: 'sig';
}

interface JwksResponse {
  keys: EcJwk[];
}

function buildJwksResponse(signingKey: SigningKeyMaterial): JwksResponse {
  const jwk = signingKey.publicKey.export({ format: 'jwk' }) as JsonWebKey;
  if (!jwk.x || !jwk.y) {
    throw new Error('signing key missing EC coordinates');
  }

  return {
    keys: [
      {
        kty: 'EC',
        crv: 'P-256',
        x: jwk.x,
        y: jwk.y,
        kid: signingKey.kid,
        alg: 'ES256',
        use: 'sig',
      },
    ],
  };
}
