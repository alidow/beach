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
import { Sheet, SheetContent, SheetHeader, SheetTitle } from './components/ui/sheet';
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
                className="gap-1.5 text-xs text-slate-300 hover:text-slate-100"
                onClick={() => setInfoOpen((prev) => !prev)}
                aria-expanded={infoOpen}
              >
                Session details
                <ChevronDown
                  className={cn('size-4 transition-transform duration-200', infoOpen ? 'rotate-180' : '')}
                />
              </Button>
            </div>
          </div>
          {infoOpen ? (
            <div className="mt-4 hidden gap-4 border-t border-white/10 pt-4 text-sm sm:grid sm:grid-cols-2">
              {infoItems.map((item) => (
                <InfoItem key={item.label} label={item.label} value={item.value} icon={item.icon} />
              ))}
            </div>
          ) : null}
        </div>
      </header>
      <Sheet open={infoOpen && status === 'connected'} onOpenChange={setInfoOpen}>
        <SheetContent side="top" className="w-full border-white/10 bg-slate-950/95 px-6 py-6 sm:hidden">
          <SheetHeader className="text-left">
            <SheetTitle className="text-slate-100">Session details</SheetTitle>
          </SheetHeader>
          <div className="mt-4 grid gap-4 text-sm">
            {infoItems.map((item) => (
              <InfoItem key={item.label} label={item.label} value={item.value} icon={item.icon} />
            ))}
          </div>
        </SheetContent>
      </Sheet>

      <Dialog open={showModal} onOpenChange={() => {}}>
        <DialogContent className="w-[min(92vw,420px)] max-h-[min(92svh,520px)] overflow-y-auto border border-white/10 bg-slate-950/95 px-7 py-8 backdrop-blur">
          <DialogHeader className="space-y-3">
            <DialogTitle>Connect to Beach</DialogTitle>
            <DialogDescription>
              Enter the session credentials from your host to begin streaming the terminal.
            </DialogDescription>
          </DialogHeader>
          <form
            className="space-y-5"
            onSubmit={(event) => {
              event.preventDefault();
              handleSubmit();
            }}
          >
            <Field label="Session ID" htmlFor="session-id-v2">
              <Input
                id="session-id-v2"
                value={sessionId}
                onChange={(event) => setSessionId(event.target.value)}
                placeholder="00000000-0000-0000-0000-000000000000"
                autoCapitalize="none"
                autoComplete="off"
                spellCheck={false}
              />
            </Field>
            <Field label="Passcode" htmlFor="passcode-v2" hint="Optional, only if provided by the host.">
              <Input
                id="passcode-v2"
                value={passcode}
                onChange={(event) => setPasscode(event.target.value)}
                type="password"
                placeholder="••••••"
              />
            </Field>
            {showAdvanced ? (
              <div className="space-y-4 rounded-xl border border-white/10 bg-slate-950/60 p-4">
                <p className="text-xs font-medium uppercase tracking-[0.28em] text-slate-500">Advanced</p>
                <Field label="Session Server" htmlFor="session-server-v2" hint="Beach session host URL.">
                  <Input
                    id="session-server-v2"
                    value={sessionServer}
                    onChange={(event) => setSessionServer(event.target.value)}
                    autoCapitalize="none"
                    autoComplete="off"
                    inputMode="url"
                  />
                </Field>
                <Field
                  label="Fallback Cohort"
                  htmlFor="fallback-cohort-v2"
                  hint="Optional override used when requesting WebSocket fallback tokens."
                >
                  <Input
                    id="fallback-cohort-v2"
                    value={fallbackCohort}
                    onChange={(event) => setFallbackCohort(event.target.value)}
                    autoCapitalize="none"
                    autoCorrect="off"
                    spellCheck={false}
                    placeholder="private-beaches"
                  />
                </Field>
                <Field
                  label="Entitlement Proof"
                  htmlFor="fallback-entitlement-v2"
                  hint="Leave blank unless provided by support."
                >
                  <Input
                    id="fallback-entitlement-v2"
                    value={fallbackEntitlement}
                    onChange={(event) => setFallbackEntitlement(event.target.value)}
                    type="password"
                    autoCapitalize="none"
                    autoCorrect="off"
                    spellCheck={false}
                    placeholder="-----BEGIN PROOF-----"
                  />
                </Field>
                <div className="flex items-center justify-between rounded-xl border border-white/5 bg-slate-950/40 px-4 py-2">
                  <span className="text-sm text-slate-300">
                    {fallbackTelemetryOptIn ? 'Telemetry enabled' : 'Telemetry disabled'}
                  </span>
                  <Switch
                    id="fallback-telemetry-v2"
                    checked={fallbackTelemetryOptIn}
                    onCheckedChange={setFallbackTelemetryOptIn}
                  />
                </div>
              </div>
            ) : null}
            <div className="flex items-center justify-between">
              <button
                type="button"
                className="text-xs font-medium text-slate-400 underline-offset-4 hover:text-slate-200 hover:underline"
                onClick={() => setShowAdvanced((prev) => !prev)}
              >
                {showAdvanced ? 'Hide advanced' : 'Advanced settings'}
              </button>
              <Button type="submit" disabled={connectDisabled} className="min-w-[120px] gap-2">
                {isConnecting ? <Loader2 className="size-4 animate-spin" /> : null}
                {connectLabel}
              </Button>
            </div>
          </form>
          {status === 'error' ? (
            <p className="mt-3 flex items-center gap-2 rounded-lg border border-rose-500/40 bg-rose-500/10 px-3 py-2 text-xs text-rose-200">
              <ShieldAlert className="size-4" />
              Connection failed. Double-check the session details and try again.
            </p>
          ) : null}
          <p className="mt-8 text-center text-[10px] font-semibold uppercase tracking-[0.32em] text-slate-500">
            beach-web {__APP_VERSION__}
          </p>
        </DialogContent>
      </Dialog>
    </main>
  );
}

interface InfoItemProps {
  label: string;
  value: string;
  icon?: ComponentType<{ className?: string }>;
}

function InfoItem({ label, value, icon: Icon = CircleDot }: InfoItemProps): JSX.Element {
  return (
    <div className="space-y-1">
      <div className="flex items-center gap-2 text-[11px] font-semibold uppercase tracking-[0.28em] text-slate-500">
        <Icon className="size-3 text-slate-600" aria-hidden />
        {label}
      </div>
      <p className="truncate text-sm text-slate-200">{value}</p>
    </div>
  );
}

interface FieldProps {
  label: string;
  htmlFor?: string;
  children: ReactNode;
  hint?: string;
}

function Field({ label, htmlFor, children, hint }: FieldProps): JSX.Element {
  return (
    <div className="space-y-2">
      <Label
        htmlFor={htmlFor}
        className="text-[11px] font-semibold uppercase tracking-[0.32em] text-slate-400"
      >
        {label}
      </Label>
      {children}
      {hint ? <p className="text-xs text-slate-500">{hint}</p> : null}
    </div>
  );
}

function shortenSessionId(value: string): string | null {
  if (!value) {
    return null;
  }
  if (value.length <= 20) {
    return value;
  }
  return `${value.slice(0, 8)}…${value.slice(-4)}`;
}
