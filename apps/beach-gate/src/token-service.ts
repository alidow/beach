import { createHmac, randomUUID } from 'node:crypto';
import { SignJWT, jwtVerify, JWTPayload } from 'jose';
import { BeachGateConfig } from './config.js';

export interface AccessTokenContext {
  userId: string;
  email?: string;
  entitlements: string[];
  tier: string;
  profile: string;
}

export interface IssuedAccessToken {
  token: string;
  expiresAt: number;
  expiresIn: number;
}

export interface ViewerTokenContext {
  sessionId: string;
  joinCode: string;
  privateBeachId?: string | null;
  ttlSeconds?: number;
}

export interface IssuedViewerToken {
  token: string;
  expiresAt: number;
  expiresIn: number;
}

export interface VerifiedAccessToken extends JWTPayload {
  sub: string;
  entitlements: string[];
  tier: string;
  profile: string;
  email?: string;
  scope?: string;
  scp?: string[];
}

export class TokenService {
  constructor(private readonly config: BeachGateConfig) {}

  async issueAccessToken(context: AccessTokenContext): Promise<IssuedAccessToken> {
    const now = Math.floor(Date.now() / 1000);
    const expiresAtSeconds = now + this.config.accessTokenTtlSeconds;
    const scope = context.entitlements.join(' ');
    const payload: JWTPayload = {
      sub: context.userId,
      entitlements: context.entitlements,
      tier: context.tier,
      profile: context.profile,
      email: context.email,
      scope,
      scp: context.entitlements,
    };

    const token = await new SignJWT(payload)
      .setAudience(this.config.serviceAudience)
      .setIssuer(this.config.issuer)
      .setSubject(context.userId)
      .setProtectedHeader({ alg: 'ES256', kid: this.config.signingKey.kid })
      .setIssuedAt(now)
      .setExpirationTime(expiresAtSeconds)
      .sign(this.config.signingKey.privateKey);

    return {
      token,
      expiresAt: expiresAtSeconds * 1000,
      expiresIn: this.config.accessTokenTtlSeconds,
    };
  }

  async verifyAccessToken(token: string): Promise<VerifiedAccessToken> {
    const result = await jwtVerify(token, this.config.signingKey.publicKey, {
      issuer: this.config.issuer,
      audience: this.config.serviceAudience,
    });

    const payload = result.payload as VerifiedAccessToken;
    if (!payload.sub) {
      throw new Error('Token missing sub claim.');
    }

    if (!Array.isArray(payload.entitlements)) {
      throw new Error('Token missing entitlements claim.');
    }

    if (typeof payload.tier !== 'string') {
      throw new Error('Token missing tier claim.');
    }

    if (typeof payload.profile !== 'string') {
      throw new Error('Token missing profile claim.');
    }

    return payload;
  }

  async issueViewerToken(context: ViewerTokenContext): Promise<IssuedViewerToken> {
    const viewer = this.config.viewerToken;
    if (!viewer) {
      throw new Error('viewer token issuance not configured');
    }

    const now = Math.floor(Date.now() / 1000);
    const ttlSeconds = Math.max(
      1,
      Math.min(context.ttlSeconds ?? viewer.ttlSeconds, viewer.ttlSeconds),
    );
    const expiresAtSeconds = now + ttlSeconds;

    const mac = createHmac('sha256', viewer.macSecret)
      .update(`${context.sessionId}:${context.joinCode}`)
      .digest('base64url');

    const payload: JWTPayload = {
      token_type: 'viewer',
      viewer: true,
      mac,
    };
    if (context.privateBeachId) {
      payload.pb = context.privateBeachId;
    }

    const token = await new SignJWT(payload)
      .setAudience(viewer.audience)
      .setIssuer(this.config.issuer)
      .setSubject(context.sessionId)
      .setProtectedHeader({ alg: 'ES256', kid: this.config.signingKey.kid })
      .setIssuedAt(now)
      .setExpirationTime(expiresAtSeconds)
      .setJti(randomUUID())
      .sign(this.config.signingKey.privateKey);

    return {
      token,
      expiresAt: expiresAtSeconds * 1000,
      expiresIn: ttlSeconds,
    };
  }
}
