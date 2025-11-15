import { readFileSync, existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import { createHash, createPrivateKey, createPublicKey, generateKeyPairSync, KeyObject } from 'node:crypto';

export interface SigningKeyMaterial {
  privateKey: KeyObject;
  publicKey: KeyObject;
  kid: string;
}

export interface EntitlementSeed {
  tier?: string;
  entitlements?: string[];
  profile?: string;
}

export interface TurnConfig {
  urls: string[];
  realm: string;
  secret: string;
  ttlSeconds: number;
  requiredEntitlements: string[];
}

export interface ViewerTokenConfig {
  ttlSeconds: number;
  audience: string;
  macSecret: Buffer;
  serviceTokens: string[];
}

export interface BeachGateConfig {
  port: number;
  host: string;
  issuer: string;
  serviceAudience: string;
  signingKidPath?: string;
  clerkIssuerUrl?: string;
  clerkClientId?: string;
  clerkClientSecret?: string;
  clerkAudience?: string;
  mockClerk: boolean;
  signingKey: SigningKeyMaterial;
  accessTokenTtlSeconds: number;
  refreshTokenTtlSeconds: number;
  defaultTier: string;
  defaultEntitlements: string[];
  entitlementOverrides: Record<string, EntitlementSeed>;
  defaultProfile: string;
  turn?: TurnConfig;
  viewerToken?: ViewerTokenConfig;
}

function readSigningKey(path?: string): SigningKeyMaterial {
  if (path && existsSync(path)) {
    const pem = readFileSync(path, 'utf8');
    const privateKey = createPrivateKey(pem);
    const publicKey = createPublicKey(privateKey);
    return {
      privateKey,
      publicKey,
      kid: fingerprint(publicKey),
    };
  }

  const { privateKey, publicKey } = generateKeyPairSync('ec', { namedCurve: 'P-256' });
  return {
    privateKey,
    publicKey,
    kid: fingerprint(publicKey),
  };
}

function fingerprint(key: KeyObject): string {
  const der = key.export({ type: 'spki', format: 'der' }) as Buffer;
  return createHash('sha256').update(der).digest('hex').slice(0, 16);
}

function parseEntitlementOverrides(raw?: string): Record<string, EntitlementSeed> {
  if (!raw) {
    return {};
  }

  try {
    const parsed = JSON.parse(raw) as Record<string, EntitlementSeed>;
    return parsed;
  } catch (error) {
    throw new Error(`Failed to parse BEACH_GATE_ENTITLEMENTS JSON: ${(error as Error).message}`);
  }
}

function parseList(raw?: string): string[] {
  if (!raw) {
    return [];
  }
  return raw
    .split(',')
    .map((value) => value.trim())
    .filter((value) => value.length > 0);
}

function loadTurnConfig(env: NodeJS.ProcessEnv): TurnConfig | undefined {
  const secret = env.BEACH_GATE_TURN_SECRET;
  const urls = parseList(env.BEACH_GATE_TURN_URLS);

  if (!secret || urls.length === 0) {
    return undefined;
  }

  const requiredEntitlements = parseList(env.BEACH_GATE_TURN_REQUIRED_ENTITLEMENTS);
  return {
    urls,
    realm: env.BEACH_GATE_TURN_REALM ?? 'turn.beach.sh',
    secret,
    ttlSeconds: Number.parseInt(env.BEACH_GATE_TURN_TTL ?? '120', 10),
    requiredEntitlements: requiredEntitlements.length > 0 ? requiredEntitlements : ['private-beach:turn'],
  };
}

function loadViewerTokenConfig(env: NodeJS.ProcessEnv): ViewerTokenConfig | undefined {
  const secret = env.BEACH_GATE_VIEWER_TOKEN_SECRET;
  const serviceTokens = parseList(env.BEACH_GATE_VIEWER_SERVICE_TOKENS);

  if (!secret || serviceTokens.length === 0) {
    return undefined;
  }

  let macSecret: Buffer;
  try {
    macSecret = Buffer.from(secret, 'base64');
    if (macSecret.length === 0) {
      throw new Error('empty base64 secret');
    }
  } catch {
    macSecret = Buffer.from(secret, 'utf8');
  }

  const ttlSeconds = Number.parseInt(env.BEACH_GATE_VIEWER_TOKEN_TTL ?? '120', 10);
  const audience = env.BEACH_GATE_VIEWER_TOKEN_AUDIENCE ?? 'beach-road';

  return {
    ttlSeconds,
    audience,
    macSecret,
    serviceTokens,
  };
}

export function loadConfig(env = process.env): BeachGateConfig {
  const port = Number.parseInt(env.BEACH_GATE_PORT ?? '4133', 10);
  const host = env.BEACH_GATE_HOST ?? '0.0.0.0';
  const issuer = env.BEACH_GATE_TOKEN_ISSUER ?? 'beach-gate';
  const serviceAudience = env.BEACH_GATE_SERVICE_AUDIENCE ?? 'beach-services';
  const signingKidPath = env.BEACH_GATE_SIGNING_KID_PATH;

  const mockClerk = env.CLERK_MOCK === '1' || (!env.CLERK_CLIENT_ID && !env.CLERK_CLIENT_SECRET);
  const defaultEntitlements = (env.BEACH_GATE_DEFAULT_ENTITLEMENTS ?? 'rescue:fallback')
    .split(',')
    .map((item) => item.trim())
    .filter(Boolean);

  const config: BeachGateConfig = {
    port,
    host,
    issuer,
    serviceAudience,
    signingKidPath,
    clerkIssuerUrl: env.CLERK_ISSUER_URL ?? env.CLERK_BASE_URL,
    clerkClientId: env.CLERK_CLIENT_ID,
    clerkClientSecret: env.CLERK_CLIENT_SECRET,
    clerkAudience: env.CLERK_AUDIENCE,
    mockClerk,
    signingKey: readSigningKey(env.BEACH_GATE_SIGNING_KEY_PATH),
    accessTokenTtlSeconds: Number.parseInt(env.BEACH_GATE_ACCESS_TTL ?? '300', 10),
    refreshTokenTtlSeconds: Number.parseInt(env.BEACH_GATE_REFRESH_TTL ?? '1800', 10),
    defaultTier: env.BEACH_GATE_DEFAULT_TIER ?? 'standard',
    defaultEntitlements,
    entitlementOverrides: parseEntitlementOverrides(env.BEACH_GATE_ENTITLEMENTS),
    defaultProfile: env.BEACH_GATE_DEFAULT_PROFILE ?? 'default',
    turn: loadTurnConfig(env),
    viewerToken: loadViewerTokenConfig(env),
  };

  return config;
}

export function persistSigningKid(kid: string, path?: string) {
  if (!path) {
    return;
  }
  try {
    const dir = dirname(path);
    mkdirSync(dir, { recursive: true });
    writeFileSync(path, kid, { encoding: 'utf8' });
  } catch (error) {
    console.warn(`[beach-gate] failed to write signing kid to ${path}: ${(error as Error).message}`);
  }
}
