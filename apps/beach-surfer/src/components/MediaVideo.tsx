import { useEffect, useMemo, useRef, useState } from 'react';
import type { MediaTransport } from '../transport/mediaTransport';
import { cn } from '../lib/utils';

export interface MediaVideoProps {
  transport: MediaTransport;
  className?: string;
  autoPlay?: boolean;
  muted?: boolean;
  controls?: boolean;
}

function u8eq(a: Uint8Array, off: number, b: string): boolean {
  if (off + b.length > a.length) return false;
  for (let i = 0; i < b.length; i += 1) {
    if (a[off + i] !== b.charCodeAt(i)) return false;
  }
  return true;
}

function looksLikeFmp4Init(bytes: Uint8Array): boolean {
  // size(4) 'ftyp'(4) ... then expect 'moov' later
  return bytes.length >= 12 && u8eq(bytes, 4, 'ftyp');
}

function looksLikeFmp4Fragment(bytes: Uint8Array): boolean {
  // search for 'moof' near the start
  const limit = Math.min(bytes.length - 4, 64 * 1024);
  for (let i = 0; i < limit; i += 1) {
    if (u8eq(bytes, i, 'moof') || u8eq(bytes, i, 'mdat')) return true;
  }
  return false;
}

function parseAvcCodecFromInit(bytes: Uint8Array): string | null {
  // naive search for 'avcC' box and read profile/compat/level
  const limit = bytes.length - 12;
  for (let i = 0; i < limit; ) {
    if (i + 8 > bytes.length) break;
    const size = (bytes[i] << 24) | (bytes[i + 1] << 16) | (bytes[i + 2] << 8) | bytes[i + 3];
    const typeOff = i + 4;
    if (u8eq(bytes, typeOff, 'avcC')) {
      const base = i + 8;
      if (base + 4 <= bytes.length) {
        const profile = bytes[base + 1];
        const compat = bytes[base + 2];
        const level = bytes[base + 3];
        const toHex = (v: number) => v.toString(16).toUpperCase().padStart(2, '0');
        return `avc1.${toHex(profile)}${toHex(compat)}${toHex(level)}`;
      }
      return null;
    }
    if (!Number.isFinite(size) || size <= 0) break;
    i += size;
  }
  return null;
}

export function MediaVideo(props: MediaVideoProps): JSX.Element {
  const { transport, className, autoPlay = true, muted = true, controls = false } = props;
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const mediaSourceRef = useRef<MediaSource | null>(null);
  const sourceBufferRef = useRef<SourceBuffer | null>(null);
  const queueRef = useRef<Uint8Array[]>([]);
  const pendingInitRef = useRef<Uint8Array | null>(null);
  const [ready, setReady] = useState(false);
  const [codec, setCodec] = useState<string | null>(null);

  const objectUrl = useMemo(() => {
    const ms = new MediaSource();
    mediaSourceRef.current = ms;
    return URL.createObjectURL(ms);
  }, []);

  useEffect(() => {
    const ms = mediaSourceRef.current!;
    const handleOpen = () => {
      setReady(true);
      // Wait for init segment to arrive to determine codec and add SourceBuffer
      flush();
    };
    ms.addEventListener('sourceopen', handleOpen);
    return () => {
      ms.removeEventListener('sourceopen', handleOpen);
      try {
        URL.revokeObjectURL(objectUrl);
      } catch {}
    };
  }, [objectUrl]);

  useEffect(() => {
    const onFrame = (event: Event) => {
      const bytes = (event as CustomEvent<Uint8Array>).detail;
      if (looksLikeFmp4Init(bytes)) {
        pendingInitRef.current = bytes;
      } else if (looksLikeFmp4Fragment(bytes)) {
        queueRef.current.push(bytes);
      } else {
        // Unknown segment; ignore
      }
      flush();
    };
    transport.addEventListener('frame', onFrame as any);
    return () => {
      transport.removeEventListener('frame', onFrame as any);
    };
  }, [transport]);

  function ensureSourceBuffer(): void {
    const ms = mediaSourceRef.current;
    if (!ms || !ready) return;
    if (sourceBufferRef.current) return;
    const init = pendingInitRef.current;
    if (!init) return;
    const codecStr = parseAvcCodecFromInit(init) ?? 'avc1.42E01E';
    setCodec(codecStr);
    const mime = `video/mp4; codecs="${codecStr}"`;
    if (!MediaSource.isTypeSupported(mime)) {
      // Try baseline as a last resort
      const fallback = 'video/mp4; codecs="avc1.42E01E"';
      if (!MediaSource.isTypeSupported(fallback)) {
        // give up
        return;
      }
    }
    try {
      const sb = ms.addSourceBuffer(mime);
      sourceBufferRef.current = sb;
      sb.addEventListener('updateend', flush);
      // Append init now
      sb.appendBuffer(toAb(init));
      pendingInitRef.current = null;
    } catch {
      // failed to create SourceBuffer
    }
  }

  function flush(): void {
    const sb = sourceBufferRef.current;
    const ms = mediaSourceRef.current;
    if (!ms || ms.readyState !== 'open') return;
    if (!sb) {
      ensureSourceBuffer();
      return;
    }
    if (sb.updating) return;
    if (pendingInitRef.current) {
      try {
        const init = pendingInitRef.current;
        if (init) {
          sb.appendBuffer(toAb(init));
          pendingInitRef.current = null;
          return;
        }
      } catch {}
    }
    const next = queueRef.current.shift();
    if (!next) return;
    try {
      sb.appendBuffer(toAb(next));
    } catch {
      // failed to append; drop and continue
    }
  }

  function toAb(bytes: Uint8Array): ArrayBuffer {
    const copy = new Uint8Array(bytes.length);
    copy.set(bytes);
    return copy.buffer;
  }

  return (
    <div className={cn('relative flex h-full w-full items-center justify-center bg-black', className)}>
      <video
        ref={videoRef}
        className="max-h-full max-w-full"
        src={objectUrl}
        autoPlay={autoPlay}
        muted={muted}
        controls={controls}
        playsInline
      />
      <div className="pointer-events-none absolute bottom-2 right-3 rounded bg-black/40 px-2 py-0.5 text-[10px] text-white/90">
        {codec ?? 'mp4'}
      </div>
    </div>
  );
}
