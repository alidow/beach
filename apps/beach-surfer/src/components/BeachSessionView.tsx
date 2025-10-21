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
import type { SecureTransportSummary } from '../transport/webrtc';
import { CabanaViewer } from './CabanaViewer';
import { cn } from '../lib/utils';

export interface BeachSessionViewProps {
  sessionId?: string;
  baseUrl?: string;
  passcode?: string;
  viewerToken?: string;
  clientLabel?: string;
  autoConnect?: boolean;
  onStatusChange?: (status: TerminalStatus) => void;
  onStreamKindChange?: (mode: ViewerMode) => void;
  onSecureSummary?: (summary: SecureTransportSummary | null) => void;
  className?: string;
  showStatusBar?: boolean;
  showTopBar?: boolean;
}

export type ViewerMode = 'unknown' | 'terminal' | 'media_png' | 'media_h264';

const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

function isPng(bytes: Uint8Array): boolean {
  if (bytes.length < PNG_SIGNATURE.length) return false;
  for (let i = 0; i < PNG_SIGNATURE.length; i += 1) {
    if (bytes[i] !== PNG_SIGNATURE[i]) return false;
  }
  return true;
}

export function BeachSessionView(props: BeachSessionViewProps): JSX.Element {
  const {
    sessionId,
    baseUrl,
    passcode,
    viewerToken,
    clientLabel,
    autoConnect = false,
    onStatusChange,
    onStreamKindChange,
    onSecureSummary,
    className,
    showStatusBar = false,
    showTopBar = false,
  } = props;

  const [_status, setStatus] = useState<TerminalStatus>('idle');
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
    const token = viewerToken?.trim();
    const label = clientLabel?.trim() || 'beach-surfer';
    if (!sid || !base) return;
    if (connectionRef.current) return;

    let cancelled = false;
    notify('connecting');
    (async () => {
      try {
        const unified = await connectUnified({
          sessionId: sid,
          baseUrl: base,
          passcode: passcode?.trim() || undefined,
          viewerToken: token || undefined,
          clientLabel: label,
        });
        if (cancelled) {
          unified.close();
          return;
        }
        connectionRef.current = unified;
        const { transport } = unified.webrtc;
        webrtcRef.current = transport;

        const onMessage = (event: Event) => {
          if (sniffedRef.current) return;
          const detail = (event as CustomEvent<any>).detail;
          if (!detail || detail.payload?.kind !== 'binary') {
            return;
          }
          try {
            decodeHostFrameBinary(detail.payload.data);
            sniffedRef.current = true;
            setMode('terminal');
            onStreamKindChange?.('terminal');
            terminalTransportRef.current = new DataChannelTerminalTransport(transport, {
              replayBinaryFirst: detail.payload.data,
            });
            notify('connected');
          } catch {
            const bytes = detail.payload.data as Uint8Array;
            if (isPng(bytes)) {
              sniffedRef.current = true;
              setMode('media_png');
              onStreamKindChange?.('media_png');
              mediaTransportRef.current = new DataChannelMediaTransport(transport);
              notify('connected');
            } else if (looksLikeFmp4(bytes)) {
              sniffedRef.current = true;
              setMode('media_h264');
              onStreamKindChange?.('media_h264');
              mediaTransportRef.current = new DataChannelMediaTransport(transport);
              notify('connected');
            } else {
              // Unknown stream type; default to media handling so future codecs can extend.
              sniffedRef.current = true;
              setMode('media_png');
              onStreamKindChange?.('media_png');
              mediaTransportRef.current = new DataChannelMediaTransport(transport);
              notify('connected');
            }
          }
        };

        const sendReady = () => {
          try {
            transport.sendText('__ready__');
          } catch {}
        };
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
          onSecureSummary?.(detail);
        };
        transport.addEventListener('message', onMessage as any);
        transport.addEventListener('secure', onSecure as any);
        transport.addEventListener('open', onOpen);
        transport.addEventListener('close', onClose);
        transport.addEventListener('error', onError);
      } catch (error) {
        console.error('[beach-session-view] connect failed', error);
        notify('error');
      }
    })();

    return () => {
      cancelled = true;
      const conn = connectionRef.current;
      connectionRef.current = null;
      sniffedRef.current = false;
      setMode('unknown');
      onStreamKindChange?.('unknown');
      setSecureSummary(null);
      onSecureSummary?.(null);
      mediaTransportRef.current = null;
      terminalTransportRef.current = null;
      notify('idle');
      try {
        conn?.close();
      } catch {}
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoConnect, sessionId, baseUrl, passcode, viewerToken, clientLabel]);

  const content = useMemo(() => {
    if (mode === 'terminal' && terminalTransportRef.current) {
      return (
        <BeachTerminal
          transport={terminalTransportRef.current}
          onStatusChange={onStatusChange}
          className="h-full w-full"
          showStatusBar={showStatusBar}
          showTopBar={showTopBar}
        />
      );
    }
    if ((mode === 'media_png' || mode === 'media_h264') && mediaTransportRef.current) {
      return (
        <CabanaViewer
          transport={mediaTransportRef.current}
          codec={mode === 'media_h264' ? 'media_h264' : 'media_png'}
          secureSummary={secureSummary}
          className="h-full w-full"
        />
      );
    }
    return <div className="h-full w-full" />;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, onStatusChange, showStatusBar, showTopBar, secureSummary]);

  return (
    <div className={cn('relative h-full w-full', className)}>
      {content}
      {mode === 'terminal' && secureSummary && secureSummary.mode === 'secure' && secureSummary.verificationCode ? (
        <div className="pointer-events-none absolute top-3 right-4 z-10 rounded-xl border border-emerald-400/30 bg-emerald-500/10 px-3 py-1 text-xs text-emerald-200">
          Verified • {secureSummary.verificationCode}
        </div>
      ) : null}
    </div>
  );
}

function looksLikeFmp4(bytes: Uint8Array): boolean {
  if (bytes.length >= 12) {
    const a = bytes[4], b = bytes[5], c = bytes[6], d = bytes[7];
    if (a === 0x66 && b === 0x74 && c === 0x79 && d === 0x70) return true;
  }
  const limit = Math.min(bytes.length - 4, 64 * 1024);
  for (let i = 0; i < limit; i += 1) {
    const a = bytes[i], b = bytes[i + 1], c = bytes[i + 2], d = bytes[i + 3];
    if (a === 0x6d && b === 0x6f && c === 0x6f && d === 0x66) return true;
    if (a === 0x6d && b === 0x64 && c === 0x61 && d === 0x74) return true;
  }
  return false;
}
