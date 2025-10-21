import { useCallback, useEffect, useMemo, useState } from 'react';
import type { MediaTransport } from '../transport/mediaTransport';
import { MediaCanvas } from './MediaCanvas';
import { MediaVideo } from './MediaVideo';
import { ViewerControls } from './ViewerControls';
import type { SecureTransportSummary } from '../transport/webrtc';
import type { MediaStats, ViewerFitMode } from './viewerTypes';
import { cn } from '../lib/utils';

export type CabanaCodec = 'media_png' | 'media_h264';

export interface CabanaViewerProps {
  transport: MediaTransport;
  codec: CabanaCodec;
  secureSummary?: SecureTransportSummary | null;
  className?: string;
}

export function CabanaViewer(props: CabanaViewerProps): JSX.Element {
  const { transport, codec, secureSummary, className } = props;

  const [paused, setPaused] = useState<boolean>(false);
  const [fitMode, setFitMode] = useState<ViewerFitMode>('contain');
  const [showStats, setShowStats] = useState<boolean>(true);
  const [mediaStats, setMediaStats] = useState<MediaStats | null>(null);
  const [mediaError, setMediaError] = useState<string | null>(null);

  useEffect(() => {
    setPaused(false);
    setMediaStats(null);
    setMediaError(null);
    setFitMode('contain');
    setShowStats(true);
  }, [transport, codec]);

  const handleStats = useCallback((stats: MediaStats) => {
    setMediaStats(stats);
    setMediaError(null);
  }, []);

  const handleMediaError = useCallback((message: string) => {
    setMediaError(message);
  }, []);

  const content = useMemo(() => {
    if (codec === 'media_png') {
      return (
        <MediaCanvas
          transport={transport}
          className="h-full w-full"
          paused={paused}
          fit={fitMode}
          onStats={handleStats}
          onError={handleMediaError}
        />
      );
    }
    if (codec === 'media_h264') {
      return (
        <MediaVideo
          transport={transport}
          className="h-full w-full"
          paused={paused}
          fit={fitMode}
          onStats={handleStats}
          onError={handleMediaError}
        />
      );
    }
    return <div className="h-full w-full" />;
  }, [codec, transport, paused, fitMode, handleStats, handleMediaError]);

  const playing = !paused;
  const effectiveStats = mediaStats ?? undefined;

  const togglePlay = () => setPaused((prev) => !prev);
  const cycleFit = () =>
    setFitMode((prev) => (prev === 'contain' ? 'cover' : prev === 'cover' ? 'actual' : 'contain'));
  const toggleStats = () => setShowStats((prev) => !prev);

  return (
    <div className={cn('relative h-full w-full', className)}>
      {content}
      <ViewerControls
        playing={playing}
        fit={fitMode}
        showStats={showStats}
        stats={effectiveStats}
        mode={codec}
        error={mediaError}
        onTogglePlay={togglePlay}
        onCycleFit={cycleFit}
        onToggleStats={toggleStats}
      />
      {secureSummary && secureSummary.mode === 'secure' && secureSummary.verificationCode ? (
        <div className="pointer-events-none absolute top-3 right-4 z-10 rounded-xl border border-emerald-400/30 bg-emerald-500/10 px-3 py-1 text-xs text-emerald-200">
          Verified • {secureSummary.verificationCode}
        </div>
      ) : null}
    </div>
  );
}
