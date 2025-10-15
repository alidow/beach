import { readFileSync, existsSync } from 'node:fs';
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

export interface BeachGateConfig {
  port: number;
  host: string;
  issuer: string;
  serviceAudience: string;
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

export function loadConfig(env = process.env): BeachGateConfig {
  const port = Number.parseInt(env.BEACH_GATE_PORT ?? '4133', 10);
  const host = env.BEACH_GATE_HOST ?? '0.0.0.0';
  const issuer = env.BEACH_GATE_TOKEN_ISSUER ?? 'beach-gate';
  const serviceAudience = env.BEACH_GATE_SERVICE_AUDIENCE ?? 'beach-services';

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
  };

  return config;
}
