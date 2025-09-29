import { type ComponentType, useMemo, useState } from 'react';
import { CircleDot, Plug, SatelliteDish } from 'lucide-react';

import { BeachTerminal, type TerminalStatus } from './components/BeachTerminal';
import { Button } from './components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from './components/ui/card';
import { Input } from './components/ui/input';
import { Label } from './components/ui/label';
import { Switch } from './components/ui/switch';

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
    tone: 'bg-slate-800/70 text-slate-200 ring-1 ring-slate-700/70',
    description: 'Awaiting connection details',
    icon: CircleDot,
  },
  connecting: {
    label: 'Negotiating',
    tone: 'bg-amber-500/20 text-amber-200 ring-1 ring-amber-400/60',
    description: 'Establishing WebRTC session…',
    icon: SatelliteDish,
  },
  connected: {
    label: 'Connected',
    tone: 'bg-emerald-500/20 text-emerald-200 ring-1 ring-emerald-400/70',
    description: 'Terminal is streaming in real-time',
    icon: Plug,
  },
  error: {
    label: 'Error',
    tone: 'bg-rose-500/20 text-rose-200 ring-1 ring-rose-400/70',
    description: 'Check your session details and try again',
    icon: CircleDot,
  },
  closed: {
    label: 'Disconnected',
    tone: 'bg-slate-800/70 text-slate-300 ring-1 ring-slate-700/70',
    description: 'Session closed by host',
    icon: CircleDot,
  },
};

export default function App(): JSX.Element {
  const [sessionId, setSessionId] = useState('');
  const [baseUrl, setBaseUrl] = useState('http://127.0.0.1:8080');
  const [passcode, setPasscode] = useState('');
  const [autoConnect, setAutoConnect] = useState(false);
  const [status, setStatus] = useState<TerminalStatus>('idle');

  const statusInfo = useMemo(() => statusStyles[status] ?? statusStyles.idle, [status]);
  const StatusIcon = statusInfo.icon;

  return (
    <main className="min-h-screen w-full bg-[hsl(var(--background))] pb-16 pt-12">
      <div className="mx-auto flex w-full max-w-6xl flex-col gap-12 px-6 lg:px-8">
        <header className="flex flex-col gap-6">
          <div className="flex flex-wrap items-center justify-between gap-4">
            <span className="inline-flex items-center gap-2 rounded-full border border-[hsl(var(--border))]/70 bg-[hsl(var(--card))]/50 px-4 py-1 text-xs uppercase tracking-[0.28em] text-[hsl(var(--muted-foreground))]">
              <span className="size-1.5 rounded-full bg-[hsl(var(--accent))]" />
              Experimental client
            </span>
            <Button variant="ghost" size="sm" className="text-[hsl(var(--muted-foreground))] hover:text-[hsl(var(--foreground))]">
              View Docs
            </Button>
          </div>
          <div className="max-w-2xl space-y-3">
            <h1 className="text-4xl font-semibold text-[hsl(var(--foreground))] lg:text-5xl">Beach Terminal</h1>
            <CardDescription className="text-base">
              A WebRTC-powered terminal experience that feels as polished as a native app—tuned for clarity,
              low-latency typing and long-lived sessions.
            </CardDescription>
          </div>
        </header>

        <div className="grid gap-8 lg:grid-cols-[320px_minmax(0,1fr)]">
          <Card className="border border-[hsl(var(--border))]/80 bg-[hsl(var(--card))]/70">
            <CardHeader>
              <CardTitle>Connection</CardTitle>
              <CardDescription>Configure the beach session you want to ride.</CardDescription>
            </CardHeader>
            <CardContent className="space-y-7">
              <div className="space-y-4">
                <Label htmlFor="session-id">
                  Session ID
                  <Input
                    id="session-id"
                    value={sessionId}
                    onChange={(event) => setSessionId(event.target.value)}
                    placeholder="00000000-0000-0000-0000-000000000000"
                    autoCapitalize="none"
                    autoComplete="off"
                  />
                </Label>
                <Label htmlFor="base-url">
                  Base URL
                  <Input
                    id="base-url"
                    value={baseUrl}
                    onChange={(event) => setBaseUrl(event.target.value)}
                    placeholder="http://127.0.0.1:8080"
                    autoCapitalize="none"
                    autoComplete="off"
                    inputMode="url"
                  />
                </Label>
                <Label htmlFor="passcode">
                  Passcode
                  <Input
                    id="passcode"
                    value={passcode}
                    onChange={(event) => setPasscode(event.target.value)}
                    type="password"
                    placeholder="optional"
                  />
                </Label>
              </div>

              <div className="flex items-center justify-between rounded-xl border border-dashed border-[hsl(var(--border))]/70 bg-[hsl(var(--card))]/60 px-4 py-3">
                <div className="space-y-1 text-xs">
                  <span className="font-semibold tracking-[0.18em] text-[hsl(var(--muted-foreground))]">AUTO CONNECT</span>
                  <p className="text-[hsl(var(--muted-foreground))]">
                    Automatically dial the session when details are filled.
                  </p>
                </div>
                <Switch checked={autoConnect} onCheckedChange={setAutoConnect} aria-label="Auto connect" />
              </div>

              <div className="rounded-xl border border-[hsl(var(--border))]/50 bg-[hsl(var(--terminal-bezel))]/60 p-4">
                <p className="text-sm font-semibold uppercase tracking-[0.32em] text-[hsl(var(--muted-foreground))]">
                  Status
                </p>
                <div className="mt-3 flex items-center gap-3 text-sm text-[hsl(var(--muted-foreground))]">
                  <span className={`inline-flex items-center gap-2 rounded-full px-3 py-1 text-xs font-medium ${statusInfo.tone}`}>
                    <StatusIcon className="size-3" />
                    {statusInfo.label}
                  </span>
                  <span>{statusInfo.description}</span>
                </div>
              </div>
            </CardContent>
          </Card>

          <section className="relative rounded-[28px] border border-[hsl(var(--terminal-frame))]/90 bg-[hsl(var(--terminal-frame))]/80 shadow-terminal-glow">
            <div className="absolute inset-0 -z-10 rounded-[28px] bg-terminal-gradient opacity-40 blur-xl" aria-hidden />
            <div className="flex items-center justify-between gap-4 rounded-t-[28px] border-b border-[hsl(var(--terminal-bezel))]/80 bg-[hsl(var(--terminal-bezel))]/70 px-8 py-4">
              <div className="flex items-center gap-2">
                <span className="size-3.5 rounded-full bg-[#ff5f57] shadow-inner shadow-black/40" />
                <span className="size-3.5 rounded-full bg-[#febc2e] shadow-inner shadow-black/40" />
                <span className="size-3.5 rounded-full bg-[#28c840] shadow-inner shadow-black/40" />
              </div>
              <div className="flex items-center gap-2 text-sm font-medium tracking-[0.3em] text-[hsl(var(--muted-foreground))]">
                <span>BEACH</span>
                <span aria-hidden>·</span>
                <span>WEB</span>
              </div>
              <div className="flex items-center gap-2 text-xs text-[hsl(var(--muted-foreground))]">
                <span className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-medium ${statusInfo.tone}`}>
                  <StatusIcon className="size-3" />
                  {statusInfo.label}
                </span>
              </div>
            </div>
            <div className="relative flex min-h-[480px] flex-col gap-0 rounded-b-[28px] border-t border-transparent bg-[hsl(var(--terminal-screen))]/95">
              <div className="pointer-events-none absolute inset-x-0 top-0 h-12 bg-gradient-to-b from-slate-900/60 to-transparent" aria-hidden />
              <div className="relative flex-1 rounded-b-[28px] p-6">
                <BeachTerminal
                  sessionId={sessionId || undefined}
                  baseUrl={baseUrl || undefined}
                  passcode={passcode || undefined}
                  autoConnect={autoConnect}
                  className="h-full"
                  showStatusBar={false}
                  onStatusChange={setStatus}
                />
              </div>
            </div>
          </section>
        </div>
      </div>
    </main>
  );
}
