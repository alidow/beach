import dynamic from 'next/dynamic';
import type { SessionTerminalPreviewClientProps } from './SessionTerminalPreviewClient';

const SessionTerminalPreviewClient = dynamic(
  () =>
    import('./SessionTerminalPreviewClient').then(
      (mod) => mod.SessionTerminalPreviewClient,
    ),
  { ssr: false },
);

export const SessionTerminalPreview = (props: SessionTerminalPreviewClientProps) => {
  return <SessionTerminalPreviewClient {...props} />;
};
