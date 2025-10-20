import { useEffect, useRef, useState } from 'react';
import type { MediaTransport } from '../transport/mediaTransport';
import { cn } from '../lib/utils';
import type { MediaStats, ViewerFitMode } from './viewerTypes';

export interface MediaCanvasProps {
  transport: MediaTransport;
  className?: string;
  paused?: boolean;
  fit?: ViewerFitMode;
  onStats?: (stats: MediaStats) => void;
  onError?: (message: string) => void;
}

/**
 * Canvas-based renderer for PNG frames arriving over the data channel.
 * Decodes the most recent frame and drops older ones under load.
 */
export function MediaCanvas(props: MediaCanvasProps): JSX.Element {
  const { transport, className, paused = false, fit = 'contain', onStats, onError } = props;
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const nextFrameRef = useRef<Uint8Array | null>(null);
  const decodeInFlightRef = useRef<boolean>(false);
  const disposedRef = useRef<boolean>(false);
  const pausedRef = useRef<boolean>(paused);
  const pendingWhenPausedRef = useRef<Uint8Array | null>(null);
  const [dimensions, setDimensions] = useState<{ width: number; height: number } | null>(null);
  const framesRef = useRef<number>(0);
  const bytesRef = useRef<number>(0);
  const lastReportRef = useRef<{ time: number; frames: number; bytes: number }>({
    time: performance.now(),
    frames: 0,
    bytes: 0,
  });

  useEffect(() => {
    const handleFrame = (event: Event) => {
      const bytes = (event as CustomEvent<Uint8Array>).detail;
      bytesRef.current += bytes.byteLength;
      framesRef.current += 1;
      if (pausedRef.current) {
        pendingWhenPausedRef.current = bytes;
        maybeReportStats();
        return;
      }
      nextFrameRef.current = bytes;
      if (!decodeInFlightRef.current) {
        decodeInFlightRef.current = true;
        void decodeAndDrawLatest();
      }
    };

    const handleClose = () => {
      // no-op, canvas stays
    };

    const handleError = () => {
      // no-op for now
    };

    transport.addEventListener('frame', handleFrame as any);
    transport.addEventListener('close', handleClose);
    transport.addEventListener('error', handleError);
    return () => {
      disposedRef.current = true;
      transport.removeEventListener('frame', handleFrame as any);
      transport.removeEventListener('close', handleClose);
      transport.removeEventListener('error', handleError);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [transport]);

  useEffect(() => {
    if (pausedRef.current === paused) return;
    pausedRef.current = paused;
    if (!paused && pendingWhenPausedRef.current && !decodeInFlightRef.current) {
      nextFrameRef.current = pendingWhenPausedRef.current;
      pendingWhenPausedRef.current = null;
      decodeInFlightRef.current = true;
      void decodeAndDrawLatest();
    }
  }, [paused]);

  async function decodeAndDrawLatest(): Promise<void> {
    if (disposedRef.current) {
      return;
    }
    const bytes = nextFrameRef.current;
    nextFrameRef.current = null;
    if (!bytes) {
      decodeInFlightRef.current = false;
      return;
    }
    try {
      // Ensure an ArrayBuffer-backed copy (avoid SharedArrayBuffer typing issues)
      const copy = new Uint8Array(bytes.byteLength);
      copy.set(bytes);
      const blob = new Blob([copy.buffer], { type: 'image/png' });
      const bitmap = await createImageBitmap(blob);
      if (disposedRef.current) {
        bitmap.close();
        return;
      }
      const canvas = canvasRef.current;
      if (!canvas) {
        bitmap.close();
        decodeInFlightRef.current = false;
        return;
      }
      if (!dimensions || dimensions.width !== bitmap.width || dimensions.height !== bitmap.height) {
        setDimensions({ width: bitmap.width, height: bitmap.height });
        canvas.width = bitmap.width;
        canvas.height = bitmap.height;
      }
      const ctx = canvas.getContext('2d');
      if (ctx) {
        ctx.clearRect(0, 0, canvas.width, canvas.height);
        ctx.drawImage(bitmap, 0, 0);
      }
      bitmap.close();
    } catch {
      onError?.('Failed to decode PNG frame');
    } finally {
      if (nextFrameRef.current && !disposedRef.current && !pausedRef.current) {
        // More frames pending; decode the most recent.
        void decodeAndDrawLatest();
      } else {
        decodeInFlightRef.current = false;
      }
      maybeReportStats();
    }
  }

  function maybeReportStats(): void {
    if (!onStats) return;
    const now = performance.now();
    const elapsed = now - lastReportRef.current.time;
    if (elapsed < 500) {
      return;
    }
    const framesDelta = framesRef.current - lastReportRef.current.frames;
    const bytesDelta = bytesRef.current - lastReportRef.current.bytes;
    lastReportRef.current = { time: now, frames: framesRef.current, bytes: bytesRef.current };
    const seconds = elapsed / 1000;
    const fps = framesDelta > 0 && seconds > 0 ? framesDelta / seconds : undefined;
    const bitrateKbps = bytesDelta > 0 && seconds > 0 ? (bytesDelta * 8) / (seconds * 1000) : undefined;
    onStats({
      mode: 'png',
      frames: framesRef.current,
      width: dimensions?.width,
      height: dimensions?.height,
      fps,
      bitrateKbps,
      bytes: bytesRef.current,
      timestamp: now,
    });
  }

  const fitClass =
    fit === 'cover' ? 'object-cover' : fit === 'actual' ? 'object-none' : 'object-contain';
  const canvasStyle =
    fit === 'actual'
      ? {
          width: dimensions?.width ? `${dimensions.width}px` : undefined,
          height: dimensions?.height ? `${dimensions.height}px` : undefined,
        }
      : { width: '100%', height: '100%' };

  return (
    <div className={cn('relative flex h-full w-full items-center justify-center bg-black', className)}>
      <canvas
        ref={canvasRef}
        className={cn('max-h-full max-w-full select-none', fitClass)}
        style={{ imageRendering: 'auto', ...canvasStyle }}
      />
    </div>
  );
}
