import { type ComponentType, useEffect, useMemo, useRef, useState } from 'react';
import {
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  CircleDot,
  Loader2,
  Plug,
  SatelliteDish,
  Settings2,
} from 'lucide-react';

import { BeachTerminal, type TerminalStatus } from './components/BeachTerminal';
import { Button } from './components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from './components/ui/card';
import { Input } from './components/ui/input';
import { Label } from './components/ui/label';
import { cn } from './lib/utils';

const statusStyles: Record<
  TerminalStatus,
  {
    label: string;
    tone: string;
    description: string;
    icon: ComponentType<{ className?: string }>;
  }
> = {
  idle: {
    label: 'Idle',
    tone: 'bg-slate-800/60 text-slate-200 ring-1 ring-slate-700/70',
    description: 'Ready for a new session.',
    icon: CircleDot,
  },
  connecting: {
    label: 'Connecting',
    tone: 'bg-amber-500/20 text-amber-100 ring-1 ring-amber-400/60',
    description: 'Negotiating with the host…',
    icon: SatelliteDish,
  },
  connected: {
    label: 'Connected',
    tone: 'bg-emerald-500/20 text-emerald-100 ring-1 ring-emerald-400/60',
    description: 'Streaming terminal output in real-time.',
    icon: Plug,
  },
  error: {
    label: 'Error',
    tone: 'bg-rose-500/25 text-rose-200 ring-1 ring-rose-400/60',
    description: 'Check the session details and try again.',
    icon: CircleDot,
  },
  closed: {
    label: 'Disconnected',
    tone: 'bg-slate-800/60 text-slate-300 ring-1 ring-slate-700/70',
    description: 'Session closed by host.',
    icon: CircleDot,
  },
};

export default function App(): JSX.Element {
  const [sessionId, setSessionId] = useState('');
  const [sessionServer, setSessionServer] = useState('http://127.0.0.1:8080');
  const [passcode, setPasscode] = useState('');
  const [status, setStatus] = useState<TerminalStatus>('idle');
  const [connectRequested, setConnectRequested] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [panelCollapsed, setPanelCollapsed] = useState(false);
  const [terminalFullscreen, setTerminalFullscreen] = useState(false);

  const collapseTimerRef = useRef<number | null>(null);
  const autoCollapsePendingRef = useRef(false);
  const previousStatusRef = useRef<TerminalStatus>("idle");
  const panelWasCollapsedRef = useRef(false);

  const statusInfo = useMemo(() => statusStyles[status] ?? statusStyles.idle, [status]);
  const StatusIcon = statusInfo.icon;
  const trimmedSessionId = sessionId.trim();
  const trimmedServer = sessionServer.trim();
  const isConnecting = status === 'connecting';
  const connectDisabled = !trimmedSessionId || !trimmedServer || isConnecting;
  const connectLabel = isConnecting ? 'Connecting…' : status === 'connected' ? 'Reconnect' : 'Connect';

  useEffect(() => {
    if (status === 'error' || status === 'closed') {
      setConnectRequested(false);
    }
  }, [status]);

  useEffect(() => {
    if (terminalFullscreen) {
      if (collapseTimerRef.current !== null) {
        window.clearTimeout(collapseTimerRef.current);
        collapseTimerRef.current = null;
      }
      autoCollapsePendingRef.current = false;
      previousStatusRef.current = status;
      return;
    }

    if (collapseTimerRef.current !== null) {
      window.clearTimeout(collapseTimerRef.current);
      collapseTimerRef.current = null;
    }
    if (status === 'connected' && autoCollapsePendingRef.current) {
      collapseTimerRef.current = window.setTimeout(() => {
        setPanelCollapsed(true);
        autoCollapsePendingRef.current = false;
      }, 900);
    } else {
      autoCollapsePendingRef.current = false;
    }

    if (previousStatusRef.current === 'connected' && status !== 'connected') {
      setPanelCollapsed(false);
    }

    previousStatusRef.current = status;
  }, [status, terminalFullscreen]);

  useEffect(() => () => {
    if (collapseTimerRef.current !== null) {
      window.clearTimeout(collapseTimerRef.current);
    }
  }, []);

  useEffect(() => {
    if (!terminalFullscreen) {
      return;
    }
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => {
      document.body.style.overflow = previousOverflow;
    };
  }, [terminalFullscreen]);

  const handleConnect = (): void => {
    if (connectDisabled) {
      return;
    }
    if (!terminalFullscreen) {
      setPanelCollapsed(false);
    }
    autoCollapsePendingRef.current = true;
    setConnectRequested(true);
  };

  const handleToggleFullscreen = (next: boolean): void => {
    if (next) {
      // Save current state before collapsing
      panelWasCollapsedRef.current = panelCollapsed;
      // Always collapse when going fullscreen
      setPanelCollapsed(true);
      autoCollapsePendingRef.current = false;
    } else {
      // When exiting fullscreen, keep collapsed if it was open before (auto-collapse behavior)
      // Only restore to open if it was already collapsed before fullscreen
      setPanelCollapsed(true);
    }
    setTerminalFullscreen(next);
  };

  const toggleAdvanced = (): void => {
    setShowAdvanced((previous) => !previous);
  };

  const reopenPanel = (): void => {
    setPanelCollapsed(false);
  };

  const collapsePanel = (): void => {
    setPanelCollapsed(true);
    autoCollapsePendingRef.current = false;
    setTerminalFullscreen(false);
  };

  return (
    <main className="min-h-screen bg-[hsl(var(--background))] text-[hsl(var(--foreground))]">
      <div className="mx-auto flex min-h-screen w-full max-w-6xl flex-col gap-10 px-6 pb-16 pt-14 lg:px-10">
        <div className="relative w-full">
          <aside
            className={cn(
              'absolute inset-x-0 top-0 z-30 mx-auto w-full max-w-xl transition-all duration-500 ease-out',
              'lg:left-0 lg:right-auto lg:mx-0 lg:w-[360px] lg:max-w-none',
              terminalFullscreen && 'hidden',
              panelCollapsed
                ? '-translate-y-[120%] opacity-0 pointer-events-none lg:translate-y-0 lg:-translate-x-[120%]'
                : 'translate-y-0 opacity-100 lg:translate-x-0',
            )}
            aria-hidden={panelCollapsed || terminalFullscreen}
          >
            <Card className="w-full rounded-3xl border border-[hsl(var(--border))]/80 bg-[hsl(var(--card))]/80 backdrop-blur">
              <CardHeader className="p-8 pb-4">
                <div className="flex items-start justify-between gap-4">
                  <div className="space-y-1.5">
                    <CardTitle className="text-xl font-semibold text-[hsl(var(--foreground))]">Connection</CardTitle>
                    <CardDescription className="text-sm text-[hsl(var(--muted-foreground))]">
                      Provide your session credentials and connect when you’re ready.
                    </CardDescription>
                  </div>
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={collapsePanel}
                    aria-label="Hide session panel"
                    className="text-[hsl(var(--muted-foreground))] hover:text-[hsl(var(--foreground))]"
                  >
                    <ChevronLeft className="size-4" />
                  </Button>
                </div>
              </CardHeader>
              <CardContent className="space-y-7 px-8 pb-10">
                <div className="space-y-5">
                  <Field label="Session ID" htmlFor="session-id">
                    <Input
                      id="session-id"
                      value={sessionId}
                      onChange={(event) => setSessionId(event.target.value)}
                      placeholder="00000000-0000-0000-0000-000000000000"
                      autoCapitalize="none"
                      autoComplete="off"
                      spellCheck={false}
                    />
                  </Field>
                  <Field label="Passcode" htmlFor="passcode" hint="Optional. Leave blank unless provided by the host.">
                    <Input
                      id="passcode"
                      value={passcode}
                      onChange={(event) => setPasscode(event.target.value)}
                      type="password"
                      placeholder="••••••"
                    />
                  </Field>
                </div>

                <div className="space-y-3">
                  <Button type="button" onClick={handleConnect} disabled={connectDisabled} className="w-full gap-2">
                    {isConnecting ? (
                      <>
                        <Loader2 className="size-4 animate-spin" />
                        <span>{connectLabel}</span>
                      </>
                    ) : (
                      connectLabel
                    )}
                  </Button>
                  <div className="rounded-2xl border border-[hsl(var(--border))]/60 bg-[hsl(var(--terminal-bezel))]/45 px-5 py-4">
                    <p className="text-xs font-semibold uppercase tracking-[0.4em] text-[hsl(var(--muted-foreground))]">
                      Status
                    </p>
                    <div className="mt-3 flex flex-wrap items-center gap-3 text-sm text-[hsl(var(--muted-foreground))]">
                      <span className={`inline-flex items-center gap-2 rounded-full px-3 py-1 text-xs font-semibold ${statusInfo.tone}`}>
                        <StatusIcon className="size-3" />
                        {statusInfo.label}
                      </span>
                      <span>{statusInfo.description}</span>
                    </div>
                  </div>
                </div>

                <div className="space-y-3">
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={toggleAdvanced}
                    className="h-auto gap-2 px-0 text-[hsl(var(--muted-foreground))] hover:text-[hsl(var(--foreground))]"
                  >
                    <Settings2 className="size-4" />
                    Advanced
                    <ChevronDown
                      className={cn('size-4 transition-transform', showAdvanced ? 'rotate-180' : 'rotate-0')}
                    />
                  </Button>
                  {showAdvanced ? (
                    <div className="space-y-3 rounded-2xl border border-[hsl(var(--border))]/60 bg-[hsl(var(--card))]/60 px-5 py-4">
                      <Field label="Session Server" htmlFor="session-server" hint="The Beach session host URL.">
                        <Input
                          id="session-server"
                          value={sessionServer}
                          onChange={(event) => setSessionServer(event.target.value)}
                          autoCapitalize="none"
                          autoComplete="off"
                          inputMode="url"
                        />
                      </Field>
                    </div>
                  ) : null}
                </div>
              </CardContent>
            </Card>
          </aside>

          {terminalFullscreen ? (
            <div className="fixed inset-0 z-30 bg-[#020617]/80 backdrop-blur-sm transition-opacity" aria-hidden />
          ) : null}
          <div
            className={cn(
              terminalFullscreen
                ? 'fixed inset-0 z-40 flex items-center justify-center px-4 py-6 sm:px-10'
                : 'transition-all duration-500 ease-out',
              !terminalFullscreen && (panelCollapsed ? 'lg:pl-0' : 'lg:pl-[400px]')
            )}
          >
            <div
              className={cn(
                'relative mx-auto flex w-full flex-col rounded-3xl border border-[hsl(var(--terminal-frame))]/90 bg-[hsl(var(--terminal-frame))]/80 shadow-[0_40px_120px_-45px_rgba(10,140,255,0.35)] transition-[height,width,transform] duration-500 ease-out',
                terminalFullscreen ? 'h-[min(95vh,900px)] max-w-[1100px] rounded-[28px]' : ''
              )}
            >
              {panelCollapsed && !terminalFullscreen ? (
                <div className="absolute left-0 top-6 z-20 -translate-x-1/2">
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={reopenPanel}
                    className="inline-flex h-8 w-8 items-center justify-center rounded-full border border-white/10 bg-[hsl(var(--card))]/70 text-[hsl(var(--muted-foreground))] backdrop-blur transition hover:bg-[hsl(var(--card))]/85"
                    aria-label="Show session panel"
                  >
                    <ChevronRight className="size-4" />
                  </Button>
                </div>
              ) : null}
              <div className="relative flex-1 rounded-b-3xl bg-[hsl(var(--terminal-screen))]/95 p-6">
                <BeachTerminal
                  sessionId={trimmedSessionId || undefined}
                  baseUrl={trimmedServer || undefined}
                  passcode={passcode || undefined}
                  autoConnect={connectRequested}
                  className="h-full"
                  showStatusBar={false}
                  onStatusChange={setStatus}
                  isFullscreen={terminalFullscreen}
                  onToggleFullscreen={handleToggleFullscreen}
                />
              </div>
            </div>
          </div>
        </div>
      </div>
    </main>
  );
}

interface FieldProps {
  label: string;
  htmlFor: string;
  children: React.ReactNode;
  hint?: string;
}

function Field({ label, htmlFor, children, hint }: FieldProps): JSX.Element {
  return (
    <div className="space-y-2">
      <Label htmlFor={htmlFor} className="text-[11px] font-semibold uppercase tracking-[0.32em]">
        {label}
      </Label>
      {children}
      {hint ? <p className="text-xs text-[hsl(var(--muted-foreground))]">{hint}</p> : null}
    </div>
  );
}
