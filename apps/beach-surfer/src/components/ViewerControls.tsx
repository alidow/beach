import { memo } from 'react';
import type { MediaStats, ViewerFitMode } from './viewerTypes';

export interface ViewerControlsProps {
  playing: boolean;
  fit: ViewerFitMode;
  showStats: boolean;
  stats?: MediaStats;
  mode: 'media_png' | 'media_h264';
  error?: string | null;
  onTogglePlay(): void;
  onCycleFit(): void;
  onToggleStats(): void;
}

export const ViewerControls = memo(function ViewerControls(props: ViewerControlsProps): JSX.Element {
  const { playing, fit, showStats, stats, mode, error, onTogglePlay, onCycleFit, onToggleStats } = props;

  const fitLabel =
    fit === 'contain' ? 'Contain' : fit === 'cover' ? 'Cover' : 'Actual';

  return (
    <div className="pointer-events-none absolute inset-0 flex flex-col justify-end">
      <div className="pointer-events-auto m-3 flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={onTogglePlay}
          className="rounded bg-black/60 px-3 py-1 text-xs font-medium text-white transition hover:bg-black/80"
        >
          {playing ? 'Pause' : 'Play'}
        </button>
        <button
          type="button"
          onClick={onCycleFit}
          className="rounded bg-black/60 px-3 py-1 text-xs font-medium text-white transition hover:bg-black/80"
        >
          Fit: {fitLabel}
        </button>
        <button
          type="button"
          onClick={onToggleStats}
          className="rounded bg-black/60 px-3 py-1 text-xs font-medium text-white transition hover:bg-black/80"
        >
          Stats: {showStats ? 'On' : 'Off'}
        </button>
      </div>
      {error ? (
        <div className="pointer-events-auto mx-3 mb-2 rounded border border-red-400/60 bg-red-500/20 px-3 py-2 text-xs text-red-200">
          {error}
        </div>
      ) : null}
      {showStats && stats ? (
        <div className="pointer-events-auto mx-3 mb-3 w-max min-w-[14rem] rounded border border-white/10 bg-black/60 px-4 py-3 text-xs text-white">
          <div className="mb-1 font-semibold uppercase tracking-wide text-white/80">
            {mode === 'media_h264' ? 'H.264 Stream' : 'PNG Stream'}
          </div>
          <dl className="grid grid-cols-2 gap-x-3 gap-y-1">
            {stats.width && stats.height ? (
              <>
                <dt className="text-white/60">Resolution</dt>
                <dd>{stats.width}×{stats.height}</dd>
              </>
            ) : null}
            {stats.fps ? (
              <>
                <dt className="text-white/60">FPS</dt>
                <dd>{stats.fps.toFixed(1)}</dd>
              </>
            ) : null}
            {stats.bitrateKbps ? (
              <>
                <dt className="text-white/60">Bitrate</dt>
                <dd>{stats.bitrateKbps.toFixed(1)} kbps</dd>
              </>
            ) : null}
            <>
              <dt className="text-white/60">Frames</dt>
              <dd>{stats.frames}</dd>
            </>
            {typeof stats.bufferedSeconds === 'number' ? (
              <>
                <dt className="text-white/60">Buffered</dt>
                <dd>{stats.bufferedSeconds.toFixed(2)} s</dd>
              </>
            ) : null}
            {stats.codec ? (
              <>
                <dt className="text-white/60">Codec</dt>
                <dd>{stats.codec}</dd>
              </>
            ) : null}
          </dl>
        </div>
      ) : null}
    </div>
  );
});
