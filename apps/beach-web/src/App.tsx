import {
  type CSSProperties,
  type ComponentType,
  type ReactNode,
  useEffect,
  useMemo,
  useState,
} from 'react';
import { ChevronDown, CircleDot, Loader2, Plug, Server, ShieldAlert, X } from 'lucide-react';
import { BeachTerminal, type TerminalStatus } from './components/BeachTerminal';
import { Button } from './components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from './components/ui/dialog';
import { Input } from './components/ui/input';
import { Label } from './components/ui/label';
import { Switch } from './components/ui/switch';
import { useConnectionController } from './hooks/useConnectionController';
import { useDocumentTitle } from './hooks/useDocumentTitle';
import { cn } from './lib/utils';

const SHELL_STYLE: CSSProperties = {
  minHeight: 'var(--beach-shell-height, 100dvh)',
};

const STATUS_META: Record<
  TerminalStatus,
  {
    label: string;
    tone: string;
    helper: string;
    icon: ComponentType<{ className?: string }>;
  }
> = {
  idle: {
    label: 'Idle',
    tone: 'bg-slate-800/80 text-slate-200 ring-1 ring-slate-700/60',
    helper: 'Ready to connect when you are.',
    icon: CircleDot,
  },
  connecting: {
    label: 'Connecting',
    tone: 'bg-amber-500/10 text-amber-100 ring-1 ring-amber-400/40',
    helper: 'Negotiating with the host…',
    icon: Loader2,
  },
  connected: {
    label: 'Connected',
    tone: 'bg-emerald-500/10 text-emerald-100 ring-1 ring-emerald-400/40',
    helper: 'Streaming terminal output live.',
    icon: Plug,
  },
  error: {
    label: 'Error',
    tone: 'bg-rose-500/15 text-rose-100 ring-1 ring-rose-400/40',
    helper: 'Double-check your credentials and try again.',
    icon: ShieldAlert,
  },
  closed: {
    label: 'Disconnected',
    tone: 'bg-slate-800/80 text-slate-200 ring-1 ring-slate-700/60',
    helper: 'Session closed by host.',
    icon: CircleDot,
  },
};

export default function App(): JSX.Element {
  const {
    sessionId,
    setSessionId,
    sessionServer,
    setSessionServer,
    passcode,
    setPasscode,
    status,
    connectRequested,
    requestConnect,
    cancelConnect,
    trimmedSessionId,
    trimmedServer,
    isConnecting,
    connectDisabled,
    connectLabel,
    onStatusChange,
    fallbackCohort,
    setFallbackCohort,
    fallbackEntitlement,
    setFallbackEntitlement,
    fallbackTelemetryOptIn,
    setFallbackTelemetryOptIn,
  } = useConnectionController();

  useDocumentTitle({ sessionId: trimmedSessionId });

  const [showAdvanced, setShowAdvanced] = useState(false);
  const [infoOpen, setInfoOpen] = useState(false);

  useEffect(() => {
    if (status !== 'connected') {
      setInfoOpen(false);
    }
  }, [status]);

  const statusMeta = useMemo(() => STATUS_META[status] ?? STATUS_META.idle, [status]);
  const showModal = status !== 'connected';
  const sessionPreview = useMemo(() => shortenSessionId(trimmedSessionId), [trimmedSessionId]);

  const handleSubmit = (): void => {
    if (!connectDisabled) {
      requestConnect();
    }
  };

  const handleDisconnect = (): void => {
    cancelConnect();
  };

  const infoItems: Array<{ label: string; value: string; icon?: ComponentType<{ className?: string }> }> = [
    {
      label: 'Session ID',
      value: sessionPreview ?? 'Not yet connected',
    },
    {
      label: 'Session Server',
      value: trimmedServer || 'https://api.beach.sh',
      icon: Server,
    },
  ];

  return (
    <main
      className="relative flex min-h-screen flex-col bg-slate-950 text-slate-100"
      style={SHELL_STYLE}
    >
      <div
        className="pointer-events-none absolute inset-0 -z-10 bg-[radial-gradient(circle_at_top,_rgba(15,23,42,0.75),_rgba(2,6,23,0.95))]"
        aria-hidden
      />
      <div
        className="pointer-events-none absolute inset-0 -z-10 bg-[url('data:image/svg+xml,%3Csvg width=\'48\' height=\'48\' viewBox=\'0 0 48 48\' fill=\'none\' xmlns=\'http://www.w3.org/2000/svg\'%3E%3Cpath d=\'M0 47.5H48v-1H0v1Z\' stroke=\'%2307172A\' stroke-opacity=\'0.35\' stroke-width=\'0.5\'/%3E%3Cpath d=\'M47.5 0v48h1V0h-1Z\' stroke=\'%2307172A\' stroke-opacity=\'0.35\' stroke-width=\'0.5\'/%3E%3C/svg%3E')] opacity-40"
        aria-hidden
      />

      <section className="relative flex flex-1">
        <BeachTerminal
          sessionId={trimmedSessionId || undefined}
          baseUrl={trimmedServer || undefined}
          passcode={passcode || undefined}
          autoConnect={connectRequested}
          onStatusChange={onStatusChange}
          className="flex-1"
          showStatusBar={false}
          showTopBar={false}
        />

        {status === 'idle' ? (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
            <div className="rounded-2xl border border-white/10 bg-slate-950/70 px-6 py-4 text-sm text-slate-300 shadow-xl backdrop-blur">
              Launch a session to start streaming your terminal.
            </div>
          </div>
        ) : null}
      </section>

      <header className="pointer-events-none absolute inset-x-0 top-0 z-20 flex justify-center p-5 sm:p-8">
        <div className="pointer-events-auto w-full max-w-3xl rounded-2xl border border-white/10 bg-slate-950/80 px-5 py-4 shadow-2xl backdrop-blur">
          <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
            <div className="flex items-center gap-3">
              <span
                className={cn(
                  'inline-flex items-center gap-1.5 rounded-full px-3 py-1 text-xs font-semibold',
                  statusMeta.tone,
                  status === 'connecting' && 'pl-2',
                )}
              >
                <statusMeta.icon className={cn('size-3.5', status === 'connecting' ? 'animate-spin' : '')} />
                {statusMeta.label}
              </span>
              <p className="text-sm text-slate-300">{statusMeta.helper}</p>
            </div>
            <div className="flex items-center gap-2">
              {status === 'connected' ? (
                <Button variant="outline" size="sm" onClick={handleDisconnect} className="gap-1.5">
                  <X className="size-3.5" />
                  Disconnect
                </Button>
              ) : null}
              {status === 'connecting' ? (
                <Button variant="ghost" size="sm" onClick={handleDisconnect} className="text-slate-300">
                  Cancel
                </Button>
              ) : null}
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => setInfoOpen(true)}
                className="gap-1.5 text-slate-100"
              >
                <ChevronDown className="size-3.5" />
                Session details
              </Button>
            </div>
          </div>
        </div>
      </header>

      <footer className="pointer-events-none absolute inset-x-0 bottom-0 z-20 flex justify-center p-5 sm:p-8">
        <div className="pointer-events-auto w-full max-w-3xl rounded-2xl border border-white/10 bg-slate-950/80 px-5 py-4 shadow-2xl backdrop-blur">
          <form
            className="flex flex-col gap-4 sm:flex-row sm:items-end sm:justify-between"
            onSubmit={(event) => {
              event.preventDefault();
              handleSubmit();
            }}
          >
            <div className="flex flex-1 flex-col gap-4 sm:flex-row">
              <div className="flex flex-1 flex-col gap-2">
                <Label htmlFor="sessionId">Session ID</Label>
                <Input
                  id="sessionId"
                  value={sessionId}
                  onChange={(event) => setSessionId(event.target.value)}
                  placeholder="Open Beach and copy the Session ID"
                  autoComplete="off"
                  autoCorrect="off"
                  autoCapitalize="off"
                  spellCheck={false}
                  inputMode="numeric"
                  className="bg-slate-900/70"
                />
              </div>
              <div className="flex flex-1 flex-col gap-2">
                <Label htmlFor="passcode">Passcode</Label>
                <Input
                  id="passcode"
                  value={passcode}
                  onChange={(event) => setPasscode(event.target.value)}
                  placeholder="Optional"
                  autoComplete="off"
                  autoCorrect="off"
                  autoCapitalize="off"
                  spellCheck={false}
                  className="bg-slate-900/70"
                />
              </div>
            </div>

            <div className="flex flex-col gap-2 sm:w-36">
              <Label>&nbsp;</Label>
              <Button
                type="submit"
                size="lg"
                className="h-11 gap-2"
                disabled={connectDisabled}
              >
                {isConnecting ? <Loader2 className="size-4 animate-spin" /> : null}
                {connectLabel}
              </Button>
            </div>
          </form>

          <div className="mt-4 flex flex-col gap-4 text-sm text-slate-300">
            <button
              type="button"
              className="flex items-center gap-2 text-left text-xs uppercase tracking-wide text-slate-400 transition hover:text-slate-200"
              onClick={() => setShowAdvanced((value) => !value)}
            >
              <span className="font-medium">{showAdvanced ? 'Hide advanced settings' : 'Show advanced settings'}</span>
              <span aria-hidden>{showAdvanced ? '−' : '+'}</span>
            </button>

            {showAdvanced ? (
              <div className="grid gap-3 sm:grid-cols-2">
                <div className="flex flex-col gap-2">
                  <Label htmlFor="server">Session Server</Label>
                  <Input
                    id="server"
                    value={sessionServer}
                    onChange={(event) => setSessionServer(event.target.value)}
                    placeholder="Defaults to https://api.beach.sh"
                    autoComplete="off"
                    autoCorrect="off"
                    autoCapitalize="off"
                    spellCheck={false}
                    className="bg-slate-900/70"
                  />
                </div>

                <div className="flex items-center justify-between gap-4 rounded-lg border border-white/10 bg-slate-900/50 p-3">
                  <div className="flex flex-col">
                    <Label htmlFor="fallback-cohort" className="text-xs uppercase tracking-wide text-slate-400">
                      Cohort
                    </Label>
                    <Input
                      id="fallback-cohort"
                      value={fallbackCohort}
                      onChange={(event) => setFallbackCohort(event.target.value)}
                      placeholder="Optional cohort label"
                      autoComplete="off"
                      autoCorrect="off"
                      autoCapitalize="off"
                      spellCheck={false}
                      className="bg-slate-950/40"
                    />
                  </div>
                  <div className="flex flex-col">
                    <Label htmlFor="fallback-entitlement" className="text-xs uppercase tracking-wide text-slate-400">
                      Entitlement
                    </Label>
                    <Input
                      id="fallback-entitlement"
                      value={fallbackEntitlement}
                      onChange={(event) => setFallbackEntitlement(event.target.value)}
                      placeholder="Optional"
                      autoComplete="off"
                      autoCorrect="off"
                      autoCapitalize="off"
                      spellCheck={false}
                      className="bg-slate-950/40"
                    />
                  </div>
                </div>

                <div className="flex items-center justify-between gap-4 rounded-lg border border-white/10 bg-slate-900/50 p-3 sm:col-span-2">
                  <div>
                    <p className="text-xs uppercase tracking-wide text-slate-400">Telemetry</p>
                    <p className="text-sm text-slate-300">
                      Allow anonymous diagnostics to improve Beach reliability.
                    </p>
                  </div>
                  <Switch
                    id="fallback-telemetry"
                    checked={fallbackTelemetryOptIn}
                    onCheckedChange={(value: boolean) => setFallbackTelemetryOptIn(value)}
                  />
                </div>
              </div>
            ) : null}
          </div>
        </div>
      </footer>

      <Dialog open={showModal && infoOpen} onOpenChange={(next) => setInfoOpen(next)}>
        <DialogContent className="w-full max-w-lg border-white/10 bg-slate-950/95 text-slate-100">
          <DialogHeader>
            <DialogTitle>Session details</DialogTitle>
            <DialogDescription>
              Keep this information private while you&apos;re connected.
            </DialogDescription>
          </DialogHeader>

          <ul className="grid gap-3">
            {infoItems.map((item) => (
              <InfoListItem key={item.label} icon={item.icon} label={item.label}>
                {item.value}
              </InfoListItem>
            ))}
            <InfoListItem label="Status">
              {statusMeta.label}
            </InfoListItem>
          </ul>

          {status !== 'connected' ? (
            <div className="flex justify-end">
              <Button variant="ghost" onClick={() => setInfoOpen(false)}>
                Close
              </Button>
            </div>
          ) : null}
        </DialogContent>
      </Dialog>
    </main>
  );
}

interface InfoListItemProps {
  label: string;
  icon?: ComponentType<{ className?: string }>;
  children: ReactNode;
}

function InfoListItem({ label, icon: Icon, children }: InfoListItemProps): JSX.Element {
  return (
    <li className="flex items-center gap-3 rounded-lg border border-white/10 bg-slate-900/40 p-4 text-sm">
      {Icon ? <Icon className="size-5 text-slate-400" aria-hidden /> : null}
      <div className="flex flex-col">
        <span className="text-xs uppercase tracking-wide text-slate-400">{label}</span>
        <span className="text-slate-100">{children}</span>
      </div>
    </li>
  );
}

function shortenSessionId(value: string | null): string | null {
  if (!value) {
    return null;
  }
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  if (trimmed.length <= 12) {
    return trimmed;
  }
  return `${trimmed.slice(0, 6)}…${trimmed.slice(-4)}`;
}
