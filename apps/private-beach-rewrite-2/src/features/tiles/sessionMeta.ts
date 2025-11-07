import type { SessionSummary } from '@private-beach/shared-api';
import type { TileSessionMeta } from './types';

export function sessionSummaryToTileMeta(session: SessionSummary): TileSessionMeta {
  const metadata = session.metadata;
  let title: string | null = null;
  if (metadata && typeof metadata === 'object') {
    const record = metadata as Record<string, unknown>;
    if (typeof record.title === 'string') {
      title = record.title as string;
    } else if (typeof record.name === 'string') {
      title = record.name as string;
    }
  }
  return {
    sessionId: session.session_id,
    title: title ?? session.session_id,
    harnessType: session.harness_type ?? null,
    status: 'attached',
    pendingActions: session.pending_actions ?? 0,
  };
}

export function extractTileLinkFromMetadata(metadata: unknown): string | null {
  if (!metadata || typeof metadata !== 'object') {
    return null;
  }
  const record = metadata as Record<string, unknown>;
  const direct = record.rewrite_tile_id;
  const camel = record.rewriteTileId;
  if (typeof direct === 'string' && direct.trim().length > 0) {
    return direct.trim();
  }
  if (typeof camel === 'string' && camel.trim().length > 0) {
    return camel.trim();
  }
  return null;
}

export function buildSessionMetadataWithTile(
  base: unknown,
  tileId: string,
  sessionMeta: TileSessionMeta,
): Record<string, unknown> {
  const metadata = base && typeof base === 'object' ? { ...(base as Record<string, unknown>) } : {};
  metadata.rewrite_tile_id = tileId;
  metadata.rewrite_tile_session_meta = sessionMeta;
  return metadata;
}
