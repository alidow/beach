import { useCallback, useEffect, useMemo, useState } from 'react';
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from './ui/sheet';
import { Button } from './ui/button';
import { Badge } from './ui/badge';
import type { ShowcasePreflightResponse } from '../lib/api';
import { getShowcasePreflight } from '../lib/api';

type PongShowcaseDrawerProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  privateBeachId: string;
  token: string | null;
  managerUrl?: string;
};

const CLI_COMMAND_BASE = './apps/private-beach/demo/pong/tools/pong-stack.sh start';
const HELP_TEXT =
  'Resolve the checklist items, then run the CLI command below from the repo root to launch the demo stack.';

export default function PongShowcaseDrawer({
  open,
  onOpenChange,
  privateBeachId,
  token,
  managerUrl,
}: PongShowcaseDrawerProps) {
  const [preflight, setPreflight] = useState<ShowcasePreflightResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const hasToken = Boolean(token && token.trim().length > 0);
  const command = useMemo(() => `${CLI_COMMAND_BASE} ${privateBeachId || '<private-beach-id>'}`.trim(), [privateBeachId]);
  const blocked = preflight?.status === 'blocked';
  const issues = preflight?.issues ?? [];

  const fetchPreflight = useCallback(
    async (refresh = false) => {
      if (!open || !privateBeachId) {
        return;
      }
      if (!hasToken) {
        setError('Sign in with a manager token to run preflight checks.');
        setPreflight(null);
        return;
      }
      setLoading(true);
      setError(null);
      try {
        const result = await getShowcasePreflight(privateBeachId, token, managerUrl, refresh);
        setPreflight(result);
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setError(message || 'Unable to run showcase preflight.');
        setPreflight(null);
      } finally {
        setLoading(false);
      }
    },
    [hasToken, managerUrl, open, privateBeachId, token],
  );

  useEffect(() => {
    if (!open) {
      setCopied(false);
      return;
    }
    void fetchPreflight(false);
  }, [fetchPreflight, open]);

  const handleRefresh = useCallback(() => {
    void fetchPreflight(true);
  }, [fetchPreflight]);

  const handleCopyCommand = useCallback(async () => {
    if (typeof navigator === 'undefined' || !navigator.clipboard) {
      setCopied(false);
      return;
    }
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setCopied(false);
    }
  }, [command]);

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="w-full max-w-xl overflow-y-auto">
        <SheetHeader>
          <SheetTitle>Pong Showcase Preflight</SheetTitle>
          <SheetDescription>
            Validate required accounts, layout tiles, and controller pairings before launching the local demo stack.
          </SheetDescription>
        </SheetHeader>
        <div className="mt-6 space-y-4 text-sm">
          {!privateBeachId && (
            <p className="rounded-lg border border-border/70 bg-muted/40 px-3 py-2 text-xs">
              Select a private beach to run the showcase diagnostics.
            </p>
          )}
          {error && (
            <p className="rounded-lg border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-800 dark:text-red-100">
              {error}
            </p>
          )}
          {loading && (
            <p className="rounded-lg border border-border/70 bg-muted/40 px-3 py-2 text-xs">Running checks…</p>
          )}
          {!loading && !error && issues.length === 0 && preflight && (
            <p className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-900 dark:text-emerald-100">
              All required accounts and pairings look good.
            </p>
          )}
          {issues.length > 0 && (
            <div className="space-y-2">
              <div className="flex items-center justify-between text-xs uppercase tracking-wide text-muted-foreground">
                <span>Checklist</span>
                {preflight?.cached ? <Badge variant="secondary">cached</Badge> : null}
              </div>
              <ul className="space-y-2">
                {issues.map((issue, index) => (
                  <li
                    key={`${issue.code}-${index}`}
                    className="rounded-xl border border-border/80 bg-card/90 px-3 py-2 shadow-sm"
                  >
                    <div className="flex items-center gap-2 text-xs font-semibold">
                      <Badge variant={issue.severity === 'error' ? 'destructive' : 'secondary'}>{issue.severity}</Badge>
                      <span>{issue.code}</span>
                    </div>
                    <p className="mt-1 text-sm text-card-foreground">{issue.detail}</p>
                    {issue.remediation && (
                      <p className="mt-1 text-xs text-muted-foreground">{issue.remediation}</p>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}
          <div className="flex items-center gap-2">
            <Button variant="outline" size="sm" onClick={handleRefresh} disabled={loading || !privateBeachId}>
              Re-run checks
            </Button>
            <Button
              size="sm"
              onClick={() => {
                handleCopyCommand();
              }}
              disabled={blocked || !privateBeachId}
            >
              {blocked ? 'Resolve issues to start' : 'Start Showcase'}
            </Button>
          </div>
          <p className="text-xs text-muted-foreground">{HELP_TEXT}</p>
          <div className="rounded-xl border border-border/70 bg-muted/30 px-3 py-2 text-xs font-mono">
            <code>{command}</code>
          </div>
          {copied && (
            <p className="text-xs text-emerald-700 dark:text-emerald-200">Copied command to clipboard.</p>
          )}
          <p className="text-xs text-muted-foreground">
            Need steps? See <span className="font-medium">docs/helpful-commands/pong.txt</span> for full CLI instructions.
          </p>
        </div>
      </SheetContent>
    </Sheet>
  );
}
