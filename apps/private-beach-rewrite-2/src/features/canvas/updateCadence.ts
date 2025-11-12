import type { RelationshipCadenceConfig } from '../tiles/types';

export function formatCadenceSummary(cadence: RelationshipCadenceConfig): string {
  if (!cadence) {
    return 'Cadence not configured';
  }
  const parts: string[] = [];
  if (cadence.idleSummary) {
    parts.push('Idle');
  }
  if (cadence.allowChildPush) {
    parts.push('MCP');
  }
  if (cadence.pollEnabled) {
    parts.push(`Poll ${cadence.pollFrequencySeconds}s`);
  }
  return parts.length > 0 ? parts.join(' • ') : 'Cadence paused';
}

export function cadenceHasAnyPath(cadence: RelationshipCadenceConfig | undefined): boolean {
  if (!cadence) return false;
  return Boolean(cadence.idleSummary || cadence.allowChildPush || cadence.pollEnabled);
}
