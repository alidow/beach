import type {
  ControllerPairing,
  ControllerUpdateCadence,
  PairingTransportStatus,
} from './api';

export function formatCadenceLabel(cadence: ControllerUpdateCadence): string {
  switch (cadence) {
    case 'fast':
      return 'Fast';
    case 'balanced':
      return 'Balanced';
    case 'slow':
      return 'Calm';
    default:
      return cadence;
  }
}

export type TransportStatusDisplay = {
  label: string;
  variant: 'success' | 'warning' | 'muted' | 'danger';
  helper?: string | null;
};

function transportLabel(status: PairingTransportStatus | null | undefined): TransportStatusDisplay {
  if (!status) {
    return { label: 'Pending', variant: 'muted' };
  }
  if (status.last_error) {
    return { label: 'Error', variant: 'danger', helper: status.last_error };
  }
  const latency = Number.isFinite(status.latency_ms) ? Math.round(status.latency_ms ?? 0) : null;
  const latencySuffix = latency !== null ? ` · ${latency} ms` : '';
  switch (status.transport) {
    case 'fast_path':
      return { label: `Fast-path${latencySuffix}`, variant: 'success' };
    case 'http_fallback':
      return { label: `HTTP fallback${latencySuffix}`, variant: 'warning' };
    case 'pending':
    default:
      return { label: `Pending${latencySuffix}`, variant: 'muted' };
  }
}

export function pairingStatusDisplay(pairing: ControllerPairing): TransportStatusDisplay {
  return transportLabel(pairing.transport_status);
}

export function formatRelativeTimestamp(timestampMs?: number | null): string | null {
  if (!timestampMs) return null;
  const diff = Date.now() - timestampMs;
  if (!Number.isFinite(diff) || diff < 0) return null;
  const seconds = Math.floor(diff / 1000);
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d ago`;
  const weeks = Math.floor(days / 7);
  return `${weeks}w ago`;
}
