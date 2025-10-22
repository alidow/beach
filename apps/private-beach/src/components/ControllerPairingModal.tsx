import { useEffect, useMemo, useState } from 'react';
import {
  type ControllerPairing,
  type ControllerUpdateCadence,
  type SessionSummary,
  CONTROLLER_UPDATE_CADENCE_OPTIONS,
} from '../lib/api';
import { Dialog } from './ui/dialog';
import { Select } from './ui/select';
import { Button } from './ui/button';

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  mode: 'create' | 'edit';
  controller: SessionSummary | null;
  child: SessionSummary | null;
  pairing: ControllerPairing | null;
  sessions: SessionSummary[];
  existingPairings: ControllerPairing[];
  onSubmit: (input: {
    controllerId: string;
    childId: string;
    promptTemplate: string;
    updateCadence: ControllerUpdateCadence;
    pairingId?: string | null;
  }) => void | Promise<void>;
  onRemove?: (pairing: ControllerPairing) => void;
  submitting: boolean;
  error: string | null;
};

function describeSession(session: SessionSummary | null) {
  if (!session) return 'Select a session';
  const prefix = session.session_id.slice(0, 8);
  const hint = session.location_hint ? ` · ${session.location_hint}` : '';
  return `${prefix} (${session.harness_type}${hint})`;
}

export default function ControllerPairingModal({
  open,
  onOpenChange,
  mode,
  controller,
  child,
  pairing,
  sessions,
  existingPairings = [],
  onSubmit,
  onRemove,
  submitting,
  error,
}: Props) {
  const [controllerId, setControllerId] = useState<string>(controller?.session_id ?? '');
  const [childId, setChildId] = useState<string>(child?.session_id ?? '');
  const [promptTemplate, setPromptTemplate] = useState<string>(pairing?.prompt_template ?? '');
  const [updateCadence, setUpdateCadence] = useState<ControllerUpdateCadence>(
    pairing?.update_cadence ?? 'balanced',
  );
  const [localError, setLocalError] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    if (!open) {
      return;
    }
    setControllerId(controller?.session_id ?? '');
  }, [controller, open]);

  useEffect(() => {
    if (!open) {
      return;
    }
    setChildId(child?.session_id ?? '');
  }, [child, open]);

  useEffect(() => {
    if (!open) {
      return;
    }
    setPromptTemplate(pairing?.prompt_template ?? '');
  }, [pairing, open]);

  useEffect(() => {
    if (!open) {
      return;
    }
    setUpdateCadence(pairing?.update_cadence ?? 'balanced');
  }, [pairing, open]);

  useEffect(() => {
    if (!open) {
      setLocalError(null);
      setDirty(false);
    } else {
      setDirty(false);
    }
  }, [open]);

  const otherSessions = useMemo(() => {
    if (!controllerId) return sessions;
    return sessions.filter((session) => session.session_id !== controllerId);
  }, [sessions, controllerId]);

  const controllerOptions = useMemo(
    () =>
      sessions.map((session) => ({
        value: session.session_id,
        label: describeSession(session),
      })),
    [sessions],
  );

  const childOptions = useMemo(
    () =>
      otherSessions.map((session) => ({
        value: session.session_id,
        label: describeSession(session),
      })),
    [otherSessions],
  );
  const hasChildOptions = childOptions.length > 0;

  const selectedExisting = useMemo(
    () =>
      existingPairings.find(
        (entry) => entry.controller_session_id === controllerId && entry.child_session_id === childId,
      ) ?? null,
    [existingPairings, controllerId, childId],
  );

  const effectivePairing = pairing ?? selectedExisting ?? null;

  useEffect(() => {
    if (!open) return;
    if (effectivePairing) {
      if (!dirty || effectivePairing.pairing_id === pairing?.pairing_id) {
        setPromptTemplate(effectivePairing.prompt_template ?? '');
        setUpdateCadence(effectivePairing.update_cadence ?? 'balanced');
      }
    } else if (!dirty) {
      setPromptTemplate('');
      setUpdateCadence('balanced');
    }
  }, [effectivePairing, dirty, open, pairing]);

  const cadenceOptions: Array<{ value: ControllerUpdateCadence; label: string; helper: string }> = [
    { value: 'fast', label: 'Fast', helper: 'Aggressive updates via fast-path when available.' },
    { value: 'balanced', label: 'Balanced', helper: 'Default cadence tuned for most controllers.' },
    { value: 'slow', label: 'Calm', helper: 'Throttle updates to reduce noise or when falling back.' },
  ];

  const disableSubmit =
    submitting || !controllerId || !childId || controllerId === childId || !updateCadence || !hasChildOptions;

  const isEditMode = mode === 'edit' || Boolean(effectivePairing);
  const submitLabel = isEditMode ? 'Save pairing' : 'Create pairing';

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!controllerId) {
      setLocalError('Select a controller session.');
      return;
    }
    if (!childId) {
      setLocalError('Select a child session to control.');
      return;
    }
    if (controllerId === childId) {
      setLocalError('Controller and child must be different sessions.');
      return;
    }
    setLocalError(null);
    await onSubmit({
      controllerId,
      childId,
      promptTemplate,
      updateCadence,
      pairingId: effectivePairing?.pairing_id ?? null,
    });
  };

  const footer = (
    <div className="flex items-center justify-between gap-2">
      <div className="flex items-center gap-2">
        <Button type="button" variant="ghost" onClick={() => onOpenChange(false)} disabled={submitting}>
          Cancel
        </Button>
        {onRemove && effectivePairing && (
          <Button
            type="button"
            variant="danger"
            onClick={() => onRemove(effectivePairing)}
            disabled={submitting}
          >
            Remove pairing
          </Button>
        )}
      </div>
      <Button type="submit" form="controller-pairing-form" disabled={disableSubmit}>
        {submitting ? 'Saving…' : submitLabel}
      </Button>
    </div>
  );

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={mode === 'edit' ? 'Edit controller pairing' : 'New controller pairing'}
      description="Drag and drop tiles or use these controls to pair a controller with a child session."
      footer={footer}
    >
      <form id="controller-pairing-form" className="space-y-4" onSubmit={handleSubmit}>
        <div className="space-y-1">
          <label htmlFor="controller-select" className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Controller session
          </label>
          <Select
            value={controllerId}
            onChange={(value) => {
              setControllerId(value);
              setChildId('');
              setDirty(false);
            }}
            options={[{ value: '', label: 'Select controller' }, ...controllerOptions]}
            className="w-full"
            disabled={mode === 'edit'}
            id="controller-select"
          />
          <p className="text-[11px] text-muted-foreground">
            The controller issues prompts or actions that will drive another session.
          </p>
        </div>
        <div className="space-y-1">
          <label htmlFor="child-select" className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Child session
          </label>
          <Select
            value={childId}
            onChange={(value) => {
              setChildId(value);
              setDirty(false);
            }}
            options={[{ value: '', label: 'Select child to control' }, ...childOptions]}
            className="w-full"
            id="child-select"
          />
          <p className="text-[11px] text-muted-foreground">
            The selected session will be directed by the controller according to the template and cadence.
          </p>
          {controllerId && !hasChildOptions && (
            <p className="text-[11px] text-amber-600">
              Add another session to this beach to complete the pairing.
            </p>
          )}
        </div>
        <div className="space-y-1">
          <label htmlFor="prompt-input" className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Prompt template
          </label>
          <textarea
            id="prompt-input"
            className="h-24 w-full resize-none rounded-md border border-input bg-background p-2 text-sm text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 ring-offset-background"
            placeholder="Describe how the controller should steer the child session…"
            value={promptTemplate}
            onChange={(event) => {
              setPromptTemplate(event.target.value);
              setDirty(true);
            }}
          />
          <p className="text-[11px] text-muted-foreground">
            Optional. Leave blank to let the harness use its default behaviour.
          </p>
        </div>
        <fieldset className="space-y-2">
          <legend className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Update cadence
          </legend>
          <div className="space-y-1">
            {cadenceOptions.map((option) => {
              const disabled = !CONTROLLER_UPDATE_CADENCE_OPTIONS.includes(option.value);
              return (
                <label key={option.value} className="flex cursor-pointer items-start gap-2 rounded-md border border-border/60 bg-muted/10 p-2 text-sm">
                  <input
                    type="radio"
                    name="update-cadence"
                    className="mt-1 accent-primary"
                    value={option.value}
                    checked={updateCadence === option.value}
                    onChange={() => {
                      setUpdateCadence(option.value);
                      setDirty(true);
                    }}
                    disabled={disabled}
                  />
                  <div>
                    <div className="font-medium">{option.label}</div>
                    <div className="text-[11px] text-muted-foreground">{option.helper}</div>
                  </div>
                </label>
              );
            })}
          </div>
        </fieldset>
        {(localError || error) && (
          <div className="rounded border border-red-200/80 bg-red-500/10 p-2 text-xs text-red-600 dark:text-red-400">
            {localError || error}
          </div>
        )}
      </form>
    </Dialog>
  );
}
