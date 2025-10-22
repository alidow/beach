import { pairingStatusDisplay, formatCadenceLabel, formatRelativeTimestamp } from '../lib/pairings';
import type { ControllerPairing, SessionSummary } from '../lib/api';
import { Badge } from './ui/badge';
import { Button } from './ui/button';

type Props = {
  pairings: ControllerPairing[];
  sessions: Map<string, SessionSummary>;
  onCreate: () => void;
  onEdit: (pairing: ControllerPairing) => void;
  onRemove: (pairing: ControllerPairing) => void;
};

function shortId(id: string) {
  return id.slice(0, 8);
}

function truncate(value: string, max = 120) {
  if (value.length <= max) return value;
  return `${value.slice(0, max - 1)}…`;
}

export default function ControllerPairingSummary({ pairings, sessions, onCreate, onEdit, onRemove }: Props) {
  return (
    <div className="mt-4 rounded-lg border border-border bg-card text-card-foreground shadow-sm">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border px-4 py-3">
        <div>
          <h3 className="text-sm font-semibold">Controller Pairings</h3>
          <p className="text-xs text-muted-foreground">
            Review controller relationships, tweak prompts, or confirm fast-path status.
          </p>
        </div>
        <Button size="sm" variant="outline" onClick={onCreate}>
          New pairing
        </Button>
      </div>
      {pairings.length === 0 ? (
        <div className="px-4 py-6 text-sm text-muted-foreground">
          No active pairings yet. Drag a controller tile onto a child or use the button above to create one.
        </div>
      ) : (
        <ul className="divide-y divide-border/70">
          {pairings.map((pairing) => {
            const controller = sessions.get(pairing.controller_session_id) ?? null;
            const child = sessions.get(pairing.child_session_id) ?? null;
            const status = pairingStatusDisplay(pairing);
            const cadence = formatCadenceLabel(pairing.update_cadence);
            const updated =
              formatRelativeTimestamp(pairing.transport_status?.last_event_ms) ??
              formatRelativeTimestamp(pairing.updated_at_ms) ??
              formatRelativeTimestamp(pairing.created_at_ms);
            return (
              <li
                key={pairing.pairing_id ?? `${pairing.controller_session_id}|${pairing.child_session_id}`}
                className="flex flex-col gap-3 px-4 py-3 md:flex-row md:items-center md:justify-between"
              >
                <div className="space-y-1">
                  <div className="flex flex-wrap items-center gap-2 text-sm font-medium">
                    <span>
                      {shortId(pairing.controller_session_id)} → {shortId(pairing.child_session_id)}
                    </span>
                    <Badge variant={status.variant}>{status.label}</Badge>
                    <Badge variant="muted">{cadence}</Badge>
                    {updated && <Badge variant="muted">Updated {updated}</Badge>}
                  </div>
                  <div className="text-xs text-muted-foreground">
                    {controller ? controller.harness_type : 'Controller'} controlling{' '}
                    {child ? child.harness_type : 'child'} session.
                  </div>
                  {status.helper && (
                    <div className="text-xs text-destructive">
                      {status.helper}
                    </div>
                  )}
                  {pairing.prompt_template && (
                    <div className="text-xs text-muted-foreground">
                      Template: <span className="italic">“{truncate(pairing.prompt_template)}”</span>
                    </div>
                  )}
                </div>
                <div className="flex items-center gap-2">
                  <Button size="sm" variant="outline" onClick={() => onEdit(pairing)}>
                    Configure
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => onRemove(pairing)}>
                    Remove
                  </Button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
