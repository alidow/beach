import { memo, useMemo } from 'react';
import { useSessionTerminal } from '../hooks/useSessionTerminal';

type Props = {
  sessionId: string;
  managerUrl: string;
  token: string | null;
  className?: string;
  variant?: 'preview' | 'full';
};

function formatLines(lines: string[]): string[] {
  if (lines.length === 0) {
    return [];
  }
  const trimmed = lines.slice(-40);
  return trimmed.map((line) => (line.endsWith('\r') ? line.slice(0, -1) : line));
}

function SessionTerminalPreviewInner({ sessionId, managerUrl, token, className, variant = 'preview' }: Props) {
  const preview = useSessionTerminal(token ? sessionId : null, managerUrl, token);
  const lines = useMemo(() => formatLines(preview.lines), [preview.lines]);

  const containerClass =
    variant === 'preview'
      ? `h-full overflow-hidden bg-neutral-950/90 ${className ?? ''}`
      : `h-full overflow-auto bg-neutral-950 ${className ?? ''}`;
  const textClass =
    variant === 'preview'
      ? 'h-full w-full select-none overflow-hidden p-2 font-mono text-[11px] leading-tight text-green-300'
      : 'min-h-full w-full overflow-auto p-4 font-mono text-sm leading-relaxed text-green-200';

  if (!token) {
    return (
      <div
        className={
          variant === 'preview'
            ? `flex h-full items-center justify-center bg-neutral-950/90 text-xs text-neutral-400 ${className ?? ''}`
            : `flex h-full items-center justify-center bg-neutral-950 text-sm text-neutral-300 ${className ?? ''}`
        }
      >
        <span>Add a manager token in Settings to stream this session.</span>
      </div>
    );
  }

  if (preview.error) {
    return (
      <div className={`flex h-full items-center justify-center bg-neutral-950/90 text-xs text-red-400 ${className ?? ''}`}>
        <span>{preview.error}</span>
      </div>
    );
  }

  if (preview.connecting) {
    return (
      <div className={`flex h-full items-center justify-center bg-neutral-950/90 text-xs text-neutral-400 ${className ?? ''}`}>
        <span>Connecting…</span>
      </div>
    );
  }

  if (lines.length === 0) {
    return (
      <div className={`flex h-full items-center justify-center bg-neutral-950/90 text-xs text-neutral-500 ${className ?? ''}`}>
        <span>No output yet</span>
      </div>
    );
  }

  return (
    <div className={containerClass}>
      <pre className={textClass}>{lines.join('\n')}</pre>
    </div>
  );
}

export const SessionTerminalPreview = memo(SessionTerminalPreviewInner);
