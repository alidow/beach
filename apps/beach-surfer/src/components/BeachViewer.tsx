import { useEffect, useMemo, useRef, useState } from 'react';
import type { TerminalStatus } from './BeachTerminal';
import { BeachTerminal } from './BeachTerminal';
import { decodeHostFrameBinary } from '../protocol/wire';
import type { TerminalTransport } from '../transport/terminalTransport';
import { DataChannelTerminalTransport } from '../transport/terminalTransport';
import type { MediaTransport } from '../transport/mediaTransport';
import { DataChannelMediaTransport } from '../transport/mediaTransport';
import { connectUnified, type UnifiedConnection } from '../viewer/connectUnified';
import type { WebRtcTransport } from '../transport/webrtc';
import { MediaCanvas } from './MediaCanvas';
import { MediaVideo } from './MediaVideo';
import type { SecureTransportSummary } from '../transport/webrtc';

export interface BeachViewerProps {
  sessionId?: string;
  baseUrl?: string;
  passcode?: string;
  autoConnect?: boolean;
  onStatusChange?: (status: TerminalStatus) => void;
  className?: string;
  showStatusBar?: boolean;
  showTopBar?: boolean;
}

type ViewerMode = 'unknown' | 'terminal' | 'media_png' | 'media_h264';

const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

function isPng(bytes: Uint8Array): boolean {
  if (bytes.length < PNG_SIGNATURE.length) return false;
  for (let i = 0; i < PNG_SIGNATURE.length; i += 1) {
    if (bytes[i] !== PNG_SIGNATURE[i]) return false;
  }
  return true;
}

export function BeachViewer(props: BeachViewerProps): JSX.Element {
  const {
    sessionId,
    baseUrl,
    passcode,
    autoConnect = false,
    onStatusChange,
    className,
    showStatusBar = false,
    showTopBar = false,
  } = props;

  const [status, setStatus] = useState<TerminalStatus>('idle');
  const [mode, setMode] = useState<ViewerMode>('unknown');
  const connectionRef = useRef<UnifiedConnection | null>(null);
  const webrtcRef = useRef<WebRtcTransport | null>(null);
  const terminalTransportRef = useRef<TerminalTransport | null>(null);
  const mediaTransportRef = useRef<MediaTransport | null>(null);
  const sniffedRef = useRef<boolean>(false);
  const [secureSummary, setSecureSummary] = useState<SecureTransportSummary | null>(null);

  const notify = (next: TerminalStatus) => {
    setStatus(next);
    onStatusChange?.(next);
  };

  useEffect(() => {
    if (!autoConnect) return;
    const sid = sessionId?.trim();
    const base = baseUrl?.trim();
    if (!sid || !base) return;
    if (connectionRef.current) return; // already connected/connecting

    let cancelled = false;
    notify('connecting');
    (async () => {
      try {
        const unified = await connectUnified({
          sessionId: sid,
          baseUrl: base,
          passcode: passcode?.trim() || undefined,
          clientLabel: 'beach-surfer',
        });
        if (cancelled) {
          unified.close();
          return;
        }
        connectionRef.current = unified;
        const { transport } = unified.webrtc;
        webrtcRef.current = transport;

        // Sniff the first binary payload to determine mode.
        const onMessage = (event: Event) => {
          if (sniffedRef.current) return;
          const detail = (event as CustomEvent<any>).detail;
          if (!detail || detail.payload?.kind !== 'binary') {
            return; // ignore text during sniffing
          }
          try {
            decodeHostFrameBinary(detail.payload.data);
            sniffedRef.current = true;
            setMode('terminal');
            const tt = new DataChannelTerminalTransport(transport, {
              replayBinaryFirst: detail.payload.data,
            });
            terminalTransportRef.current = tt;
            notify('connected');
          } catch {
            const bytes = detail.payload.data as Uint8Array;
            if (isPng(bytes)) {
              sniffedRef.current = true;
              setMode('media_png');
              const mt = new DataChannelMediaTransport(transport);
              mediaTransportRef.current = mt;
              notify('connected');
            } else if (looksLikeFmp4(bytes)) {
              sniffedRef.current = true;
              setMode('media_h264');
              const mt = new DataChannelMediaTransport(transport);
              mediaTransportRef.current = mt;
              notify('connected');
            } else {
              sniffedRef.current = true;
              // Unknown stream type; keep media path to allow custom handling in future.
              setMode('media_png');
              const mt = new DataChannelMediaTransport(transport);
              mediaTransportRef.current = mt;
              notify('connected');
            }
          }
        };

        const sendReady = () => {
          // Prompt terminal hosts to start streaming by sending the readiness sentinel.
          try {
            transport.sendText('__ready__');
          } catch {}
        };
        // If the channel is already open, send immediately to avoid race with late listener.
        if (transport.isOpen()) {
          sendReady();
        }
        const onOpen = () => {
          sendReady();
        };
        const onClose = () => {
          notify('closed');
        };
        const onError = () => {
          notify('error');
        };
        const onSecure = (event: Event) => {
          const detail = (event as CustomEvent<SecureTransportSummary>).detail;
          setSecureSummary(detail);
        };
        transport.addEventListener('message', onMessage as any);
        transport.addEventListener('secure', onSecure as any);
        transport.addEventListener('open', onOpen);
        transport.addEventListener('close', onClose);
        transport.addEventListener('error', onError);
      } catch (error) {
        console.error('[beach-viewer] connect failed', error);
        notify('error');
      }
    })();

    return () => {
      cancelled = true;
      const conn = connectionRef.current;
      connectionRef.current = null;
      sniffedRef.current = false;
      setMode('unknown');
      setSecureSummary(null);
      try {
        conn?.close();
      } catch {}
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoConnect, sessionId, baseUrl, passcode]);

  const content = useMemo(() => {
    if (mode === 'terminal' && terminalTransportRef.current) {
      return (
        <BeachTerminal
          transport={terminalTransportRef.current}
          onStatusChange={onStatusChange}
          className={className}
          showStatusBar={showStatusBar}
          showTopBar={showTopBar}
        />
      );
    }
    if (mode === 'media_png' && mediaTransportRef.current) {
      return <MediaCanvas transport={mediaTransportRef.current} />;
    }
    if (mode === 'media_h264' && mediaTransportRef.current) {
      return <MediaVideo transport={mediaTransportRef.current} />;
    }
    // Placeholder while connecting/sniffing
    return <div className={className} />;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, className, showStatusBar, showTopBar]);

  return (
    <div className={className}>
      {content}
      {secureSummary && secureSummary.mode === 'secure' && secureSummary.verificationCode ? (
        <div className="pointer-events-none absolute top-3 right-4 z-10 rounded-xl border border-emerald-400/30 bg-emerald-500/10 px-3 py-1 text-xs text-emerald-200">
          Verified • {secureSummary.verificationCode}
        </div>
      ) : null}
    </div>
  );
}

function looksLikeFmp4(bytes: Uint8Array): boolean {
  // MP4 'ftyp' at offset 4 or moof/mdat fragments
  if (bytes.length >= 12) {
    const a = bytes[4], b = bytes[5], c = bytes[6], d = bytes[7];
    if (a === 0x66 && b === 0x74 && c === 0x79 && d === 0x70) return true; // 'ftyp'
  }
  const limit = Math.min(bytes.length - 4, 64 * 1024);
  for (let i = 0; i < limit; i += 1) {
    const a = bytes[i], b = bytes[i + 1], c = bytes[i + 2], d = bytes[i + 3];
    if (a === 0x6d && b === 0x6f && c === 0x6f && d === 0x66) return true; // 'moof'
    if (a === 0x6d && b === 0x64 && c === 0x61 && d === 0x74) return true; // 'mdat'
  }
  return false;
}
