'use client';

export default function TerminalTestPage() {
  return (
    <div className="p-6 text-sm leading-relaxed text-muted-foreground">
      <p className="font-semibold text-foreground">Terminal preview harness not configured.</p>
      <p className="mt-2">
        Create <code>temp/terminal-preview-harness.tsx</code> and export a <code>TerminalPreviewHarness</code>{' '}
        component to enable this development page. Until then, this placeholder keeps the build healthy.
      </p>
    </div>
  );
}
