import type { HostFrame } from '../protocol/types';

interface TraceFrame {
  kind: string;
  ts: number;
  payload: unknown;
}

interface TraceCapture {
  frames: TraceFrame[];
}

declare global {
  interface Window {
    __BEACH_TRACE_CAPTURE__?: TraceCapture;
    __BEACH_TRACE_START__?: () => void;
    __BEACH_TRACE_DUMP__?: () => TraceFrame[];
  }
}

function getCapture(): TraceCapture | undefined {
  if (typeof window === 'undefined') {
    return undefined;
  }
  if (!window.__BEACH_TRACE_CAPTURE__) {
    window.__BEACH_TRACE_CAPTURE__ = { frames: [] };
  }
  return window.__BEACH_TRACE_CAPTURE__;
}

export function ensureTraceCaptureHelpers(): void {
  if (typeof window === 'undefined') {
    return;
  }
  if (!window.__BEACH_TRACE_START__) {
    window.__BEACH_TRACE_START__ = () => {
      window.__BEACH_TRACE_CAPTURE__ = { frames: [] };
    };
  }
  if (!window.__BEACH_TRACE_DUMP__) {
    window.__BEACH_TRACE_DUMP__ = () => {
      const capture = getCapture();
      const frames = capture ? [...capture.frames] : [];
      try {
        console.info('[beach-trace][capture]', JSON.stringify(frames));
      } catch (error) {
        console.info('[beach-trace][capture]', frames, error);
      }
      return frames;
    };
  }
}

export function captureTrace(kind: string, payload: unknown): void {
  const capture = getCapture();
  if (!capture) {
    return;
  }
  let serialized: unknown = payload;
  if (payload && typeof payload === 'object') {
    try {
      serialized = JSON.parse(
        JSON.stringify(payload, (_, value) => {
          if (
            typeof value === 'object' &&
            value !== null &&
            typeof (value as { buffer?: ArrayBuffer }).buffer === 'object' &&
            (value instanceof Uint8Array || ArrayBuffer.isView(value))
          ) {
            return Array.from(value as Uint8Array);
          }
          if (value instanceof ArrayBuffer) {
            return Array.from(new Uint8Array(value));
          }
          return value;
        }),
      );
    } catch (error) {
      serialized = { error: String(error) };
    }
  }
  capture.frames.push({ kind, ts: Date.now(), payload: serialized });
}

export function serializeHostFrame(frame: HostFrame): Record<string, unknown> {
  return JSON.parse(
    JSON.stringify(frame, (_, value) => {
      if (
        typeof value === 'object' &&
        value !== null &&
        typeof (value as { buffer?: ArrayBuffer }).buffer === 'object' &&
        (value instanceof Uint8Array || ArrayBuffer.isView(value))
      ) {
        return Array.from(value as Uint8Array);
      }
      if (value instanceof ArrayBuffer) {
        return Array.from(new Uint8Array(value));
      }
      return value;
    }),
  );
}
