import { useEffect, useMemo, useState } from 'react';
import {
  type ControllerPairing,
  type ControllerUpdateCadence,
  type SessionSummary,
  CONTROLLER_UPDATE_CADENCE_OPTIONS,
} from '../lib/api';
import { Sheet } from './ui/sheet';
import { Select } from './ui/select';
import { Button } from './ui/button';
import { Badge } from './ui/badge';
import { pairingStatusDisplay, formatRelativeTimestamp, formatCadenceLabel } from '../lib/pairings';

type Props = {
  open: boolean;
  pairing: ControllerPairing | null;
  controller: SessionSummary | null;
  child: SessionSummary | null;
  onClose: () => void;
  onSave: (input: {
    controllerId: string;
    childId: string;
    promptTemplate: string;
    updateCadence: ControllerUpdateCadence;
  }) => Promise<void> | void;
  onRemove: (input: { controllerId: string; childId: string }) => Promise<void> | void;
  saving: boolean;
  error: string | null;
};

export function AssignmentDetailPane({
  open,
  pairing,
  controller,
  child,
  onClose,
  onSave,
  onRemove,
  saving,
  error,
}: Props) {
  const [promptTemplate, setPromptTemplate] = useState<string>('');
  const [updateCadence, setUpdateCadence] = useState<ControllerUpdateCadence>('balanced');
  const [localError, setLocalError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      setLocalError(null);
    }
  }, [open]);

  useEffect(() => {
    if (!pairing) {
      setPromptTemplate('');
      setUpdateCadence('balanced');
      return;
    }
    setPromptTemplate(pairing.prompt_template ?? '');
    setUpdateCadence(pairing.update_cadence ?? 'balanced');
  }, [pairing]);

  const status = useMemo(() => (pairing ? pairingStatusDisplay(pairing) : null), [pairing]);
  const lastUpdated = useMemo(() => {
    if (!pairing) return null;
    return (
      formatRelativeTimestamp(pairing.transport_status?.last_event_ms) ??
      formatRelativeTimestamp(pairing.updated_at_ms) ??
      formatRelativeTimestamp(pairing.created_at_ms)
    );
  }, [pairing]);

  if (!open || !pairing || !controller || !child) {
    return null;
  }

  const cadenceOptions = CONTROLLER_UPDATE_CADENCE_OPTIONS.map((value) => ({
    value,
    label: formatCadenceLabel(value),
  }));

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!controller || !child) {
      setLocalError('Missing session context.');
      return;
    }
    await onSave({
      controllerId: controller.session_id,
      childId: child.session_id,
      promptTemplate,
      updateCadence,
    });
  };

  return (
    <Sheet open={open} onOpenChange={(value) => (!value ? onClose() : undefined)} side="right">
      <div className="flex h-full flex-col">
        <header className="border-b border-border px-4 py-3">
          <div className="flex items-center justify-between">
            <div>
              <h2 className="text-sm font-semibold">Assignment Details</h2>
              <p className="text-xs text-muted-foreground">
                Agent {controller.session_id.slice(0, 8)} controlling {child.session_id.slice(0, 8)}
              </p>
            </div>
            <Button size="sm" variant="ghost" onClick={onClose}>
              Close
            </Button>
          </div>
          <div className="mt-2 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            {status && <Badge variant={status.variant}>{status.label}</Badge>}
            <Badge variant="muted">{formatCadenceLabel(updateCadence)}</Badge>
            {lastUpdated && <Badge variant="muted">Updated {lastUpdated}</Badge>}
          </div>
        </header>
        <form className="flex-1 overflow-auto px-4 py-4" onSubmit={handleSubmit}>
          <div className="space-y-3">
            <section>
              <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Agent</h3>
              <p className="mt-1 text-sm text-foreground">
                <span className="font-mono text-xs">{controller.session_id.slice(0, 12)}</span>
                <span className="ml-2 text-xs text-muted-foreground">{controller.harness_type}</span>
              </p>
              <p className="text-xs text-muted-foreground">{controller.location_hint || '—'}</p>
            </section>
            <section>
              <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Application</h3>
              <p className="mt-1 text-sm text-foreground">
                <span className="font-mono text-xs">{child.session_id.slice(0, 12)}</span>
                <span className="ml-2 text-xs text-muted-foreground">{child.harness_type}</span>
              </p>
              <p className="text-xs text-muted-foreground">{child.location_hint || '—'}</p>
            </section>
            <section className="space-y-2">
              <label htmlFor="assignment-prompt" className="block text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                Prompt template
              </label>
              <textarea
                id="assignment-prompt"
                className="min-h-[120px] w-full rounded-md border border-border bg-background px-3 py-2 text-sm shadow-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                placeholder="Guardrails, instructions, or context for the agent."
                value={promptTemplate}
                onChange={(event) => setPromptTemplate(event.target.value)}
              />
              <p className="text-[11px] text-muted-foreground">Leave blank to rely on the agent default prompt.</p>
            </section>
            <section className="space-y-2">
              <label htmlFor="assignment-cadence" className="block text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                Update cadence
              </label>
              <Select
                id="assignment-cadence"
                value={updateCadence}
                onChange={(value) => {
                  if (CONTROLLER_UPDATE_CADENCE_OPTIONS.includes(value as ControllerUpdateCadence)) {
                    setUpdateCadence(value as ControllerUpdateCadence);
                  }
                }}
                options={cadenceOptions}
              />
            </section>
            {(error || localError) && (
              <div className="rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-600 dark:text-red-400">
                {localError ?? error}
              </div>
            )}
          </div>
          <div className="mt-6 flex items-center justify-between gap-2 border-t border-border pt-4">
            <Button
              type="button"
              variant="ghost"
              onClick={() =>
                onRemove({
                  controllerId: controller.session_id,
                  childId: child.session_id,
                })
              }
              disabled={saving}
            >
              Remove assignment
            </Button>
            <Button type="submit" disabled={saving}>
              {saving ? 'Saving…' : 'Save changes'}
            </Button>
          </div>
        </form>
      </div>
    </Sheet>
  );
}
