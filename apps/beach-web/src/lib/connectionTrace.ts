const TRACE_NAMESPACE = '[beach-trace][connect]';

export type ConnectionTraceOutcome = 'success' | 'error' | 'cancelled';

export interface ConnectionTraceContext {
  sessionId?: string;
  baseUrl?: string;
}

export class ConnectionTrace {
  private readonly enabled: boolean;
  private readonly startedAt: number;
  private readonly context: ConnectionTraceContext;
  private closed = false;

  constructor(context: ConnectionTraceContext = {}) {
    this.enabled = isTraceEnabled();
    this.startedAt = now();
    this.context = context;
    if (this.enabled) {
      this.emit('start', {});
    }
  }

  mark(name: string, extra: Record<string, unknown> = {}): void {
    if (!this.enabled) {
      return;
    }
    this.emit(name, extra);
  }

  finish(outcome: ConnectionTraceOutcome, extra: Record<string, unknown> = {}): void {
    if (!this.enabled || this.closed) {
      return;
    }
    this.closed = true;
    this.emit('complete', { outcome, ...extra });
  }

  isEnabled(): boolean {
    return this.enabled;
  }

  private emit(name: string, extra: Record<string, unknown>): void {
    const elapsed = Number((now() - this.startedAt).toFixed(2));
    // eslint-disable-next-line no-console
    console.debug(TRACE_NAMESPACE, name, {
      ...this.context,
      elapsed_ms: elapsed,
      ...extra,
    });
  }
}

export function createConnectionTrace(context: ConnectionTraceContext = {}): ConnectionTrace | null {
  const trace = new ConnectionTrace(context);
  return trace.isEnabled() ? trace : null;
}

function isTraceEnabled(): boolean {
  if (typeof globalThis === 'undefined') {
    return false;
  }
  const globalFlag =
    (globalThis as any).__BEACH_TRACE ??
    (globalThis as any).BEACH_TRACE ??
    (typeof window !== 'undefined' ? (window as any).__BEACH_TRACE ?? (window as any).BEACH_TRACE : undefined);
  return Boolean(globalFlag);
}

function now(): number {
  if (typeof performance !== 'undefined' && typeof performance.now === 'function') {
    return performance.now();
  }
  return Date.now();
}
