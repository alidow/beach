import {
  type CSSProperties,
  type ComponentType,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import {
  ArrowUpDown,
  ChevronDown,
  CircleDot,
  Eye,
  EyeOff,
  Loader2,
  Plug,
  Server,
  ShieldAlert,
} from 'lucide-react';
import { BeachTerminal, type TerminalStatus } from './components/BeachTerminal';
import { Button } from './components/ui/button';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from './components/ui/collapsible';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from './components/ui/dialog';
import { Input } from './components/ui/input';
import { Label } from './components/ui/label';
import { useConnectionController } from './hooks/useConnectionController';
import { useDocumentTitle } from './hooks/useDocumentTitle';
import { cn } from './lib/utils';

// Host surfaces can override --beach-shell-height to fit custom containers.
const SHELL_STYLE: CSSProperties = {
  minHeight: 'var(--beach-shell-height, 100dvh)',
  height: 'var(--beach-shell-height, 100dvh)',
};

const STATUS_META: Record<
  TerminalStatus,
  {
    label: string;
    tone: string;
    ring: string;
    accent: string;
    helper: string;
    icon: ComponentType<{ className?: string }>;
  }
> = {
  idle: {
    label: 'Idle',
    tone: 'bg-slate-800/80 text-slate-200',
    ring: 'ring-1 ring-slate-700/60',
    accent: 'text-slate-400',
    helper: 'Ready for a new session.',
    icon: CircleDot,
  },
  connecting: {
    label: 'Connecting',
    tone: 'bg-amber-500/10 text-amber-200',
    ring: 'ring-1 ring-amber-400/40',
    accent: 'text-amber-300',
    helper: 'Negotiating with the host…',
    icon: Loader2,
  },
  connected: {
    label: 'Connected',
    tone: 'bg-emerald-500/10 text-emerald-200',
    ring: 'ring-1 ring-emerald-400/40',
    accent: 'text-emerald-300',
    helper: 'Streaming terminal output in real-time.',
    icon: Plug,
  },
  error: {
    label: 'Error',
    tone: 'bg-rose-500/10 text-rose-200',
    ring: 'ring-1 ring-rose-400/40',
    accent: 'text-rose-300',
    helper: 'Check your credentials and try again.',
    icon: ShieldAlert,
  },
  closed: {
    label: 'Disconnected',
    tone: 'bg-slate-800/70 text-slate-300',
    ring: 'ring-1 ring-slate-700/60',
    accent: 'text-slate-400',
    helper: 'Session closed by host.',
    icon: CircleDot,
  },
};

const BOUNDARY_PADDING = 16;
const DOCK_MARGIN = 24;

interface Position {
  top: number;
  left: number;
}

interface LayoutMetrics {
  shellRect: DOMRect;
  panelSize: {
    width: number;
    height: number;
  };
}

export default function AppV2(): JSX.Element {
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
  } = useConnectionController();
  useDocumentTitle({ sessionId: trimmedSessionId });
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [infoExpanded, setInfoExpanded] = useState(false);
  const [infoVisible, setInfoVisible] = useState(true);
  const [infoDock, setInfoDock] = useState<'top' | 'bottom' | 'floating'>('bottom');
  const [dockPreference, setDockPreference] = useState<'top' | 'bottom'>('bottom');
  const [toolbarPosition, setToolbarPosition] = useState<{ top: number; left: number } | null>(null);
  const [isDragging, setIsDragging] = useState(false);
  const shellRef = useRef<HTMLElement | null>(null);
  const toolbarRef = useRef<HTMLDivElement | null>(null);
  const toolbarSizeRef = useRef<{ width: number; height: number } | null>(null);
  const dragStateRef = useRef<{ pointerId: number; offsetX: number; offsetY: number } | null>(null);
  const getLayoutMetrics = (): LayoutMetrics | null => {
    const shellElement = shellRef.current;
    if (!shellElement) {
      return null;
    }
    const shellRect = shellElement.getBoundingClientRect();
    const toolbarElement = toolbarRef.current;
    let panelSize = toolbarSizeRef.current;

    if (toolbarElement) {
      const rect = toolbarElement.getBoundingClientRect();
      panelSize = { width: rect.width, height: rect.height };
      toolbarSizeRef.current = panelSize;
    }

    if (!panelSize) {
      const fallbackWidth = Math.max(
        200,
        Math.min(shellRect.width - BOUNDARY_PADDING * 2, 768),
      );
      panelSize = {
        width: Number.isFinite(fallbackWidth) ? fallbackWidth : shellRect.width,
        height: 0,
      };
    }

    return { shellRect, panelSize };
  };

  const clampPosition = (desired: Position, metrics?: LayoutMetrics): Position => {
    const layout = metrics ?? getLayoutMetrics();
    if (!layout) {
      return desired;
    }
    const { shellRect, panelSize } = layout;
    const maxLeft = shellRect.width - panelSize.width - BOUNDARY_PADDING;
    const maxTop = shellRect.height - panelSize.height - BOUNDARY_PADDING;
    const maxLeftWithPadding = maxLeft < BOUNDARY_PADDING ? BOUNDARY_PADDING : maxLeft;
    const maxTopWithPadding = maxTop < BOUNDARY_PADDING ? BOUNDARY_PADDING : maxTop;

    const clampedLeft = Math.min(Math.max(desired.left, BOUNDARY_PADDING), maxLeftWithPadding);
    const clampedTop = Math.min(Math.max(desired.top, BOUNDARY_PADDING), maxTopWithPadding);

    return {
      left: Number.isFinite(clampedLeft) ? clampedLeft : BOUNDARY_PADDING,
      top: Number.isFinite(clampedTop) ? clampedTop : BOUNDARY_PADDING,
    };
  };

  const computeDockedPosition = (dock: 'top' | 'bottom', metrics?: LayoutMetrics): Position => {
    const layout = metrics ?? getLayoutMetrics();
    if (!layout) {
      return { top: DOCK_MARGIN, left: DOCK_MARGIN };
    }
    const { shellRect, panelSize } = layout;
    const baseTop =
      dock === 'top' ? DOCK_MARGIN : shellRect.height - panelSize.height - DOCK_MARGIN;
    const baseLeft = (shellRect.width - panelSize.width) / 2;
    return clampPosition({ top: baseTop, left: baseLeft }, layout);
  };

  const statusMeta = STATUS_META[status] ?? STATUS_META.idle;
  const showModal = status !== 'connected';
  const sessionPreview = useMemo(() => shortenSessionId(trimmedSessionId), [trimmedSessionId]);

  useEffect(() => {
    if (status !== 'connected') {
      setInfoExpanded(false);
    }
  }, [status]);

  useEffect(() => {
    if (!infoVisible) {
      setInfoExpanded(false);
    }
  }, [infoVisible]);

  useLayoutEffect(() => {
    if (!infoVisible && !toolbarPosition) {
      return;
    }
    const metrics = getLayoutMetrics();
    if (!metrics) {
      return;
    }

    if (infoDock === 'floating') {
      setToolbarPosition((previous) => {
        if (!previous) {
          return clampPosition(computeDockedPosition(dockPreference, metrics), metrics);
        }
        const clamped = clampPosition(previous, metrics);
        if (previous.left === clamped.left && previous.top === clamped.top) {
          return previous;
        }
        return clamped;
      });
      return;
    }

    const targetPosition = computeDockedPosition(infoDock, metrics);
    setToolbarPosition((previous) => {
      if (!previous) {
        return targetPosition;
      }
      if (previous.left === targetPosition.left && previous.top === targetPosition.top) {
        return previous;
      }
      return targetPosition;
    });
  }, [dockPreference, infoDock, infoExpanded, infoVisible, toolbarPosition]);

  useEffect(() => {
    const handleResize = (): void => {
      setToolbarPosition((previous) => {
        if (!previous) {
          return previous;
        }
        const metrics = getLayoutMetrics();
        if (!metrics) {
          return previous;
        }

        if (infoDock === 'floating') {
          const clamped = clampPosition(previous, metrics);
          if (clamped.left === previous.left && clamped.top === previous.top) {
            return previous;
          }
          return clamped;
        }

        const targetPosition = computeDockedPosition(infoDock, metrics);
        if (previous.left === targetPosition.left && previous.top === targetPosition.top) {
          return previous;
        }
        return targetPosition;
      });
    };

    window.addEventListener('resize', handleResize);
    return () => window.removeEventListener('resize', handleResize);
  }, [infoDock]);

  const handleSubmit = (): void => {
    if (connectDisabled) {
      return;
    }
    requestConnect();
  };

  const handleDisconnect = (): void => {
    cancelConnect();
  };

  const toggleDockPosition = (): void => {
    const nextPreference = dockPreference === 'top' ? 'bottom' : 'top';
    setDockPreference(nextPreference);
    setInfoDock(nextPreference);
  };

  const handleHideInfo = (): void => {
    setInfoExpanded(false);
    setInfoVisible(false);
  };

  const handleShowInfo = (): void => {
    setInfoVisible(true);
  };

  const handlePointerDown = (event: React.PointerEvent<HTMLElement>): void => {
    if (event.button !== 0) {
      return;
    }
    if (event.target instanceof Element) {
      const interactiveTarget = event.target.closest(
        'button, a, input, textarea, select, label, [data-no-drag="true"]',
      );
      if (interactiveTarget) {
        return;
      }
    }

    const metrics = getLayoutMetrics();
    const toolbarElement = toolbarRef.current;
    if (!metrics || !toolbarElement) {
      return;
    }

    event.preventDefault();
    const rect = toolbarElement.getBoundingClientRect();
    dragStateRef.current = {
      pointerId: event.pointerId,
      offsetX: event.clientX - rect.left,
      offsetY: event.clientY - rect.top,
    };
    setToolbarPosition({
      left: rect.left - metrics.shellRect.left,
      top: rect.top - metrics.shellRect.top,
    });
    setInfoDock('floating');
    setIsDragging(true);
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const handlePointerMove = (event: React.PointerEvent<HTMLElement>): void => {
    const dragState = dragStateRef.current;
    if (!dragState || dragState.pointerId !== event.pointerId) {
      return;
    }
    const metrics = getLayoutMetrics();
    if (!metrics) {
      return;
    }
    const nextPosition = {
      left: event.clientX - metrics.shellRect.left - dragState.offsetX,
      top: event.clientY - metrics.shellRect.top - dragState.offsetY,
    };
    setToolbarPosition((previous) => {
      const clamped = clampPosition(nextPosition, metrics);
      if (previous && previous.left === clamped.left && previous.top === clamped.top) {
        return previous;
      }
      return clamped;
    });
  };

  const handlePointerUp = (event: React.PointerEvent<HTMLElement>): void => {
    if (!dragStateRef.current || dragStateRef.current.pointerId !== event.pointerId) {
      return;
    }
    dragStateRef.current = null;
    setIsDragging(false);

    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }

    setToolbarPosition((previous) => {
      if (!previous) {
        return previous;
      }
      const metrics = getLayoutMetrics();
      if (!metrics) {
        return previous;
      }
      const clamped = clampPosition(previous, metrics);
      if (previous.left === clamped.left && previous.top === clamped.top) {
        return previous;
      }
      return clamped;
    });
  };

  const baseToolbarPosition = toolbarPosition ?? { top: DOCK_MARGIN, left: DOCK_MARGIN };
  const measuredToolbarWidth = toolbarSizeRef.current?.width;
  const toolbarWrapperStyle: CSSProperties = {
    top: baseToolbarPosition.top,
    left: baseToolbarPosition.left,
    visibility: toolbarPosition ? 'visible' : 'hidden',
    width:
      typeof measuredToolbarWidth === 'number' && Number.isFinite(measuredToolbarWidth)
        ? `${measuredToolbarWidth}px`
        : undefined,
  };
  const toolbarWrapperClass = cn(
    'pointer-events-none absolute',
    isDragging ? 'transition-none' : 'transition-[top,left] duration-150 ease-out',
  );
  const dockToggleLabel = dockPreference === 'top' ? 'Move connection info to bottom' : 'Move connection info to top';
  const dockRevealOffset =
    infoDock === 'floating' ? '' : dockPreference === 'top' ? '-translate-y-1.5' : 'translate-y-1.5';

  return (
    <main
      ref={shellRef}
      className="relative flex w-full flex-col overflow-hidden bg-[hsl(var(--terminal-screen))] text-slate-100"
      style={SHELL_STYLE}
    >
      <div
        className="pointer-events-none absolute inset-0 -z-10 bg-[radial-gradient(circle_at_top,_rgba(15,23,42,0.75),_rgba(2,6,23,0.95))]"
        aria-hidden
      />
      <div
        className="pointer-events-none absolute inset-0 -z-10 bg-[url('data:image/svg+xml,%3Csvg width=\'60\' height=\'60\' viewBox=\'0 0 60 60\' fill=\'none\' xmlns=\'http://www.w3.org/2000/svg\'%3E%3Cpath d=\'M0 59.5H60?v-1H0v1Z\' stroke=\'%2307172A\' stroke-opacity=\'0.4\' stroke-width=\'0.5\'/%3E%3Cpath d=\'M59.5 0v60h1V0h-1Z\' stroke=\'%2307172A\' stroke-opacity=\'0.4\' stroke-width=\'0.5\'/%3E%3C/svg%3E')] opacity-50"
        aria-hidden
      />

      <div className="flex flex-1 min-h-0 flex-col">
        <div className="relative flex flex-1 min-h-0">
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
        </div>
      </div>

      {infoVisible ? (
        <div className={toolbarWrapperClass} style={toolbarWrapperStyle}>
          <div ref={toolbarRef} className="pointer-events-auto max-w-3xl min-w-[18rem]">
            <Collapsible
              open={infoExpanded}
              onOpenChange={(open) => {
                if (status !== 'connected') {
                  setInfoExpanded(false);
                  return;
                }
                setInfoExpanded(open);
              }}
            >
              <div className="flex flex-col rounded-3xl border border-white/5 bg-slate-950/70 backdrop-blur">
                <header
                  className="flex flex-col gap-3 px-5 py-3 sm:flex-row sm:items-center sm:justify-between cursor-move"
                  onPointerDown={handlePointerDown}
                  onPointerMove={handlePointerMove}
                  onPointerUp={handlePointerUp}
                  onPointerCancel={handlePointerUp}
                  style={{
                    touchAction: 'none',
                    userSelect: isDragging ? 'none' : undefined,
                  }}
                >
                  <div className="flex items-center gap-2">
                    <span
                      className={cn(
                        'inline-flex items-center gap-1.5 rounded-full px-3 py-1 text-xs font-semibold',
                        statusMeta.tone,
                        statusMeta.ring,
                        status === 'connecting' && 'pl-2',
                      )}
                    >
                      <statusMeta.icon className={cn('size-3.5', status === 'connecting' ? 'animate-spin' : '')} />
                      {statusMeta.label}
                    </span>
                    <CollapsibleTrigger asChild disabled={status !== 'connected'}>
                      <Button
                        variant="ghost"
                        size="sm"
                        className={cn(
                          'h-8 gap-1 px-3 text-xs text-slate-300 transition cursor-pointer',
                          status === 'connected' ? 'hover:bg-white/5 hover:text-slate-100' : 'opacity-50',
                        )}
                      >
                        {infoExpanded ? 'Hide' : 'Details'}
                        <ChevronDown
                          className={cn('size-3.5 transition-transform duration-200', infoExpanded ? 'rotate-180' : 'rotate-0')}
                        />
                      </Button>
                    </CollapsibleTrigger>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      onClick={toggleDockPosition}
                      aria-label={dockToggleLabel}
                      className="h-8 w-8 cursor-pointer text-slate-300 transition hover:bg-white/5 hover:text-slate-100"
                    >
                      <ArrowUpDown className={cn('size-4', dockPreference === 'bottom' ? 'rotate-180' : '')} />
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      onClick={handleHideInfo}
                      aria-label="Hide connection info"
                      className="h-8 w-8 cursor-pointer text-slate-300 transition hover:bg-white/5 hover:text-slate-100"
                    >
                      <EyeOff className="size-4" />
                    </Button>
                  </div>
                </header>
                <CollapsibleContent className="border-t border-white/8 px-5 pb-5 pt-4">
                  <div className="max-h-[min(70vh,24rem)] overflow-y-auto pr-1">
                    <div className="flex flex-col gap-6">
                      <div className="grid gap-4 sm:grid-cols-2">
                        <InfoItem label="Session ID" value={trimmedSessionId || '—'} icon={CircleDot} />
                        <InfoItem label="Session Server" value={trimmedServer || 'https://api.beach.sh'} icon={Server} />
                      </div>
                      <p className="text-sm text-slate-400">{statusMeta.helper}</p>
                      <div className="flex flex-wrap items-center gap-3">
                        <Button variant="outline" size="sm" onClick={handleDisconnect}>
                          Disconnect
                        </Button>
                        <span className="text-xs text-slate-500">
                          Need help? Double-check your session ID or contact the host.
                        </span>
                      </div>
                    </div>
                  </div>
                </CollapsibleContent>
              </div>
            </Collapsible>
          </div>
        </div>
      ) : null}
      {!infoVisible ? (
        <div className={toolbarWrapperClass} style={toolbarWrapperStyle}>
          <div className="pointer-events-none flex justify-center">
            <div
              className={cn(
                'pointer-events-auto rounded-full bg-slate-950/80 shadow-lg backdrop-blur transition-transform duration-200',
                dockRevealOffset,
              )}
            >
              <Button
                variant="outline"
                size="icon"
                onClick={handleShowInfo}
                aria-label="Show connection info"
                className="h-11 w-11 rounded-full border-white/10 bg-transparent text-slate-200 hover:bg-white/10 focus-visible:ring-white/30 select-none cursor-pointer"
              >
                <Eye className="size-4" />
                <span className="sr-only">Show connection info</span>
              </Button>
            </div>
          </div>
        </div>
      ) : null}

      <Dialog open={showModal} onOpenChange={() => {}}>
        <DialogContent className="w-[min(94vw,420px)] max-h-[min(92svh,520px)] overflow-y-auto p-7 sm:w-[440px] sm:p-8">
          <DialogHeader className="space-y-3">
            <DialogTitle>Connect to Beach</DialogTitle>
            <DialogDescription>
              Enter the session credentials from your host to begin streaming the terminal. Everything happens live
              in this window.
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
        <Icon className="size-3.5 text-slate-600" aria-hidden />
        {label}
      </div>
      <p className="truncate text-sm text-slate-200">{value}</p>
    </div>
  );
}

interface FieldProps {
  label: string;
  htmlFor: string;
  children: ReactNode;
  hint?: string;
}

function Field({ label, htmlFor, children, hint }: FieldProps): JSX.Element {
  return (
    <div className="space-y-2">
      <Label htmlFor={htmlFor} className="text-[11px] font-semibold uppercase tracking-[0.32em] text-slate-400">
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
