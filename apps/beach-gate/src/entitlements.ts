import { BeachGateConfig, EntitlementSeed } from './config.js';

export interface EntitlementContext {
  userId: string;
  email?: string;
}

export interface EntitlementResult {
  entitlements: string[];
  tier: string;
  profile: string;
}

export class EntitlementStore {
  private readonly overrides: Map<string, EntitlementSeed>;

  constructor(private readonly config: BeachGateConfig) {
    this.overrides = new Map(
      Object.entries(config.entitlementOverrides ?? {}).map(([key, value]) => [key.toLowerCase(), value]),
    );
  }

  resolve(context: EntitlementContext): EntitlementResult {
    const override = this.lookupOverride(context);
    const entitlements = dedupe(
      (override?.entitlements?.length ? override.entitlements : this.config.defaultEntitlements).map((item) => item.trim()),
    );

    return {
      entitlements,
      tier: override?.tier ?? this.config.defaultTier,
      profile: override?.profile ?? this.config.defaultProfile,
    };
  }

  private lookupOverride(context: EntitlementContext): EntitlementSeed | undefined {
    const keys = [context.userId, context.email].filter(Boolean) as string[];
    for (const key of keys) {
      const override = this.overrides.get(key.toLowerCase());
      if (override) {
        return override;
      }
    }
    return undefined;
  }
}

function dedupe(items: string[]): string[] {
  return Array.from(new Set(items.filter(Boolean)));
}
