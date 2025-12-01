/**
 * Build ICE server list from env. Supports:
 * - NEXT_PUBLIC_ICE_SERVERS as JSON array of RTCIceServer
 * - NEXT_PUBLIC_TURN_URLS as comma-separated list, plus optional username/password
 * - NEXT_PUBLIC_DISABLE_DEFAULT_STUN to skip the Google STUN fallback
 * - NEXT_PUBLIC_ICE_TRANSPORT_POLICY or NEXT_PUBLIC_FORCE_TURN to force relay-only
 */
export function maybeParseIceServers(): RTCIceServer[] | null {
  // Prefer explicit JSON blob for full control.
  const raw = process.env.NEXT_PUBLIC_ICE_SERVERS;
  if (raw && raw.trim().length > 0) {
    try {
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed) && parsed.length > 0) {
        return parsed as RTCIceServer[];
      }
    } catch {
      // ignore parse errors and fall through to other sources
    }
  }

  // Support simple TURN urls list with shared creds.
  const turnUrls = process.env.NEXT_PUBLIC_TURN_URLS;
  if (turnUrls && turnUrls.trim().length > 0) {
    const urls = turnUrls
      .split(',')
      .map((u) => u.trim())
      .filter((u) => u.length > 0);
    if (urls.length > 0) {
      const username = process.env.NEXT_PUBLIC_TURN_USERNAME;
      const credential = process.env.NEXT_PUBLIC_TURN_PASSWORD;
      if (username && credential && username.trim().length > 0 && credential.trim().length > 0) {
        return [
          {
            urls,
            username: username.trim(),
            credential: credential.trim(),
          },
        ];
      }
      // Skip invalid TURN entries to avoid ICE config errors when creds are missing.
    }
  }

  // Allow disabling the default Google STUN fallback entirely.
  const disableDefault =
    process.env.NEXT_PUBLIC_DISABLE_DEFAULT_STUN === '1' ||
    process.env.NEXT_PUBLIC_DISABLE_DEFAULT_STUN?.toLowerCase() === 'true';
  if (disableDefault) {
    return [];
  }

  return null;
}

/**
 * Derive an optional ICE transport policy from env.
 * Supported:
 * - NEXT_PUBLIC_ICE_TRANSPORT_POLICY=relay|all (pass-through)
 * - NEXT_PUBLIC_FORCE_TURN=1|true (forces relay)
 */
export function maybeParseIceTransportPolicy(): RTCIceTransportPolicy | undefined {
  const forceTurn =
    process.env.NEXT_PUBLIC_FORCE_TURN === '1' ||
    process.env.NEXT_PUBLIC_FORCE_TURN?.toLowerCase() === 'true';
  if (forceTurn) {
    return 'relay';
  }

  const policy = process.env.NEXT_PUBLIC_ICE_TRANSPORT_POLICY?.toLowerCase();
  if (policy === 'relay' || policy === 'all') {
    return policy as RTCIceTransportPolicy;
  }

  return undefined;
}
