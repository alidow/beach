function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

export function extractSessionTitle(metadata: unknown): string | null {
  if (!isRecord(metadata)) return null;
  const raw =
    typeof metadata.title === 'string'
      ? metadata.title
      : typeof metadata.name === 'string'
        ? metadata.name
        : typeof (metadata as { display_name?: unknown }).display_name === 'string'
          ? (metadata as { display_name: string }).display_name
          : null;
  if (!raw) return null;
  const trimmed = raw.trim();
  return trimmed.length > 0 ? trimmed : null;
}

export function metadataWithSessionTitle(metadata: unknown, title: string | null): Record<string, unknown> {
  const base = isRecord(metadata) ? { ...metadata } : {};
  if (title && title.trim().length > 0) {
    base.title = title.trim();
  } else {
    delete base.title;
  }
  return base;
}
