import { randomBytes, createHash } from 'node:crypto';

export interface RefreshTokenRecord {
  userId: string;
  email?: string;
  clerkRefreshToken?: string;
  entitlements: string[];
  tier: string;
  profile: string;
  issuedAt: number;
  expiresAt: number;
}

export interface RefreshIssueResult {
  token: string;
  record: RefreshTokenRecord;
}

export class RefreshTokenStore {
  private readonly records = new Map<string, RefreshTokenRecord>();

  constructor(private readonly ttlSeconds: number) {}

  create(record: Omit<RefreshTokenRecord, 'issuedAt' | 'expiresAt'>): RefreshIssueResult {
    const token = randomBase64Url(32);
    const issuedAt = Date.now();
    const expiresAt = issuedAt + this.ttlSeconds * 1000;
    const value: RefreshTokenRecord = {
      ...record,
      issuedAt,
      expiresAt,
    };
    this.records.set(hash(token), value);
    return { token, record: value };
  }

  rotate(existingToken: string, record: RefreshTokenRecord): RefreshIssueResult {
    this.records.delete(hash(existingToken));
    return this.create({
      userId: record.userId,
      email: record.email,
      clerkRefreshToken: record.clerkRefreshToken,
      entitlements: record.entitlements,
      tier: record.tier,
      profile: record.profile,
    });
  }

  verify(token: string): RefreshTokenRecord | undefined {
    this.pruneExpired();
    return this.records.get(hash(token));
  }

  delete(token: string): void {
    this.records.delete(hash(token));
  }

  private pruneExpired(): void {
    const now = Date.now();
    for (const [key, value] of this.records) {
      if (value.expiresAt <= now) {
        this.records.delete(key);
      }
    }
  }
}

function randomBase64Url(size: number): string {
  return randomBytes(size).toString('base64url');
}

function hash(value: string): string {
  return createHash('sha256').update(value).digest('hex');
}
