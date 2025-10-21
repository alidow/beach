import dynamic from 'next/dynamic';

const TerminalPreviewHarness = dynamic(
  () => import('../../../../temp/terminal-preview-harness').then((mod) => mod.TerminalPreviewHarness),
  { ssr: false, loading: () => <div className="p-6 text-sm text-muted-foreground">Loading harness…</div> },
);

export default function TerminalTestPage() {
  return <TerminalPreviewHarness />;
}
