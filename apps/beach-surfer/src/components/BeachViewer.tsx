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

type ViewerMode = 'unknown' | 'terminal' | 'media';

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
            const tt = new DataChannelTerminalTransport(transport);
            terminalTransportRef.current = tt;
            notify('connected');
          } catch {
            const bytes = detail.payload.data as Uint8Array;
            if (isPng(bytes)) {
              sniffedRef.current = true;
              setMode('media');
              const mt = new DataChannelMediaTransport(transport);
              mediaTransportRef.current = mt;
              notify('connected');
            } else {
              sniffedRef.current = true;
              // Unknown stream type; keep media path to allow custom handling in future.
              setMode('media');
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
        transport.addEventListener('message', onMessage as any);
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
    if (mode === 'media' && mediaTransportRef.current) {
      return <MediaCanvas transport={mediaTransportRef.current} />;
    }
    // Placeholder while connecting/sniffing
    return <div className={className} />;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, className, showStatusBar, showTopBar]);

  return content;
}
