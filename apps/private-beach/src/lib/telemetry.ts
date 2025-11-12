// Lightweight telemetry shim for Private Beach canvas interactions.
// Attempts to send events to a configured sink; falls back to console.

export type TelemetryEvent =
  | 'canvas.drag.start'
  | 'canvas.drag.stop'
  | 'canvas.resize.stop'
  | 'canvas.layout.persist'
  | 'canvas.tile.create'
  | 'canvas.tile.move'
  | 'canvas.tile.remove'
  | 'canvas.resize.auto'
  | 'canvas.tile.connect.start'
  | 'canvas.tile.connect.success'
  | 'canvas.tile.connect.failure'
  | 'canvas.tile.connect.disposed'
  | 'canvas.rewrite.flag-state'
  | 'canvas.group.create'
  | 'canvas.assignment.success'
  | 'canvas.assignment.failure'
  | 'canvas.measurement'
  | 'canvas.measurement.dom-skipped-after-host'
  | 'canvas.measurement.dom-advanced-after-host';

type TelemetryPayload = Record<string, unknown> & { time?: number };

declare global {
  interface Window {
    __BEACH_TELEMETRY__?: (event: string, payload: TelemetryPayload) => void;
  }
}

const BASE_URL =
  (typeof process !== 'undefined' && (process as any).env?.NEXT_PUBLIC_TELEMETRY_BASE_URL) || '';

export function emitTelemetry(event: TelemetryEvent, payload: TelemetryPayload = {}): void {
  try {
    const body = JSON.stringify({
      event,
      time: Date.now(),
      payload,
      app: 'private-beach',
      version: 'v1',
    });

    if (typeof window !== 'undefined' && typeof window.__BEACH_TELEMETRY__ === 'function') {
      window.__BEACH_TELEMETRY__(event, { time: Date.now(), ...payload });
      return;
    }

    if (typeof navigator !== 'undefined' && typeof navigator.sendBeacon === 'function' && BASE_URL) {
      const url = `${BASE_URL.replace(/\/$/, '')}/telemetry/frontend`;
      const ok = navigator.sendBeacon(url, new Blob([body], { type: 'application/json' }));
      if (ok) return;
    }

    // Fallback to fetch in browsers or console in SSR/test
    if (typeof fetch !== 'undefined' && BASE_URL) {
      fetch(`${BASE_URL.replace(/\/$/, '')}/telemetry/frontend`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body,
        keepalive: true,
      }).catch(() => {});
      return;
    }

    // Final fallback: console logging
    // eslint-disable-next-line no-console
    console.info('[telemetry:fallback]', event, payload);
  } catch {
    // Swallow errors — telemetry must never break UX/tests
  }
}
