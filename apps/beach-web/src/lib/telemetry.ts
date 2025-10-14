const SECURE_TELEMETRY_PATH = '/telemetry/secure-transport';

export interface SecureTransportTelemetry {
  sessionId?: string;
  handshakeId?: string;
  role: 'offerer' | 'answerer';
  outcome: 'success' | 'failure' | 'fallback';
  verificationCode?: string;
  latencyMs?: number;
  reason?: string;
  client: 'web';
}

export async function reportSecureTransportEvent(
  baseUrl: string | undefined,
  payload: Omit<SecureTransportTelemetry, 'client'>,
): Promise<void> {
  if (!baseUrl) {
    return;
  }
  const url = buildTelemetryUrl(baseUrl);
  const body = JSON.stringify({ ...payload, client: 'web' });

  try {
    if (typeof navigator !== 'undefined' && typeof navigator.sendBeacon === 'function') {
      const blob = new Blob([body], { type: 'application/json' });
      const sent = navigator.sendBeacon(url, blob);
      if (sent) {
        return;
      }
    }
  } catch (error) {
    // Fallback to fetch below
    console.warn('secure transport telemetry beacon failed', error);
  }

  try {
    await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      keepalive: true,
      body,
    });
  } catch (error) {
    console.warn('secure transport telemetry fetch failed', error);
  }
}

function buildTelemetryUrl(baseUrl: string): string {
  const trimmed = baseUrl.trim();
  if (!trimmed) {
    return SECURE_TELEMETRY_PATH;
  }
  const normalised = trimmed.endsWith('/') ? trimmed.slice(0, -1) : trimmed;
  try {
    const url = new URL(normalised);
    url.pathname = `${url.pathname.replace(/\/$/, '')}${SECURE_TELEMETRY_PATH}`;
    url.search = '';
    url.hash = '';
    return url.toString();
  } catch {
    return `${normalised}${SECURE_TELEMETRY_PATH}`;
  }
}
