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

export interface VerifiedAccessToken extends JWTPayload {
  sub: string;
  entitlements: string[];
  tier: string;
  profile: string;
  email?: string;
}

export class TokenService {
  constructor(private readonly config: BeachGateConfig) {}

  async issueAccessToken(context: AccessTokenContext): Promise<IssuedAccessToken> {
    const now = Math.floor(Date.now() / 1000);
    const expiresAtSeconds = now + this.config.accessTokenTtlSeconds;
    const payload: JWTPayload = {
      sub: context.userId,
      entitlements: context.entitlements,
      tier: context.tier,
      profile: context.profile,
      email: context.email,
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
}
