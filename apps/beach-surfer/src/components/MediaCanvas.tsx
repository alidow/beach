import { useEffect, useRef, useState } from 'react';
import type { MediaTransport } from '../transport/mediaTransport';
import { cn } from '../lib/utils';

export interface MediaCanvasProps {
  transport: MediaTransport;
  className?: string;
}

/**
 * Simple canvas-based renderer for PNG frames arriving over the data channel.
 * Decodes the most recent frame and drops older ones under load.
 */
export function MediaCanvas(props: MediaCanvasProps): JSX.Element {
  const { transport, className } = props;
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const nextFrameRef = useRef<Uint8Array | null>(null);
  const decodeInFlightRef = useRef<boolean>(false);
  const disposedRef = useRef<boolean>(false);
  const [dimensions, setDimensions] = useState<{ width: number; height: number } | null>(null);

  useEffect(() => {
    const handleFrame = (event: Event) => {
      const bytes = (event as CustomEvent<Uint8Array>).detail;
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
      // no-op for MVP
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
      // ignore decode errors for MVP
    } finally {
      if (nextFrameRef.current && !disposedRef.current) {
        // More frames pending; decode the most recent.
        void decodeAndDrawLatest();
      } else {
        decodeInFlightRef.current = false;
      }
    }
  }

  return (
    <div className={cn('relative flex h-full w-full items-center justify-center bg-black', className)}>
      <canvas
        ref={canvasRef}
        className="max-h-full max-w-full select-none"
        style={{ imageRendering: 'auto' }}
      />
    </div>
  );
}
