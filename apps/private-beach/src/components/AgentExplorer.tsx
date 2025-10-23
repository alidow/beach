import { useMemo, useState } from 'react';
import type { SessionSummary, ControllerPairing } from '../lib/api';
import type { AssignmentEdge } from '../lib/assignments';
import { Badge } from './ui/badge';
import { Button } from './ui/button';
import { Select } from './ui/select';
import { pairingStatusDisplay } from '../lib/pairings';

const APPLICATION_DRAG_TYPE = 'application/x-private-beach-application';

type Props = {
  agents: SessionSummary[];
  applications: SessionSummary[];
  assignmentsByAgent: Map<string, AssignmentEdge[]>;
  assignmentsByApplication: Map<string, ControllerPairing[]>;
  onCreateAssignment: (agentId: string, applicationId: string) => void;
  onRemoveAssignment: (agentId: string, applicationId: string) => void;
  onOpenAssignment: (pairing: ControllerPairing) => void;
  selectedAgentId: string | null;
  onSelectAgent: (agentId: string | null) => void;
  selectedApplicationId: string | null;
  onSelectApplication: (applicationId: string | null) => void;
  onAddToLayout: (sessionId: string) => void;
};

export function AgentExplorer({
  agents,
  applications,
  assignmentsByAgent,
  assignmentsByApplication,
  onCreateAssignment,
  onRemoveAssignment,
  onOpenAssignment,
  selectedAgentId,
  onSelectAgent,
  selectedApplicationId,
  onSelectApplication,
  onAddToLayout,
}: Props) {
  const [assignSelections, setAssignSelections] = useState<Record<string, string>>({});

  const applicationLookup = useMemo(() => {
    const map = new Map<string, SessionSummary>();
    applications.forEach((app) => map.set(app.session_id, app));
    agents.forEach((agent) => map.set(agent.session_id, agent));
    return map;
  }, [applications, agents]);

  const handleAssignSelection = (applicationId: string, agentId: string) => {
    setAssignSelections((prev) => ({ ...prev, [applicationId]: '' }));
    if (!agentId) return;
    onCreateAssignment(agentId, applicationId);
  };

  return (
    <div className="flex h-full flex-col overflow-hidden rounded-lg border border-border bg-card text-card-foreground shadow-sm">
      <div className="border-b border-border px-3 py-3">
        <h2 className="text-sm font-semibold">Explorer</h2>
        <p className="text-xs text-muted-foreground">Drag applications onto agents or use the assign menu.</p>
      </div>
      <div className="flex-1 overflow-auto px-3 py-3">
        <section className="mb-4">
          <div className="mb-2 flex items-center justify-between text-xs uppercase tracking-wide text-muted-foreground">
            <span>Agents</span>
            <span>{agents.length}</span>
          </div>
          <ul className="space-y-2">
            {agents.length === 0 && (
              <li className="rounded border border-dashed border-border/70 px-3 py-2 text-[11px] text-muted-foreground">
                No agents yet. Convert a session to an agent from the tile menu.
              </li>
            )}
            {agents.map((agent) => {
              const assignments = assignmentsByAgent.get(agent.session_id) ?? [];
              const isSelected = selectedAgentId === agent.session_id;
              return (
                <li
                  key={agent.session_id}
                  className={`rounded border px-3 py-2 text-sm transition ${
                    isSelected ? 'border-primary bg-primary/5' : 'border-border hover:border-primary/60'
                  }`}
                  onClick={() =>
                    onSelectAgent(isSelected ? null : agent.session_id)
                  }
                  onKeyDown={(event) => {
                    if (event.key === 'Enter' || event.key === ' ') {
                      event.preventDefault();
                      onSelectAgent(isSelected ? null : agent.session_id);
                    }
                  }}
                  role="button"
                  tabIndex={0}
                  onDragOver={(event) => {
                    if (event.dataTransfer?.types.includes(APPLICATION_DRAG_TYPE)) {
                      event.preventDefault();
                    }
                  }}
                  onDrop={(event) => {
                    const applicationId = event.dataTransfer?.getData(APPLICATION_DRAG_TYPE);
                    if (applicationId) {
                      event.preventDefault();
                      onCreateAssignment(agent.session_id, applicationId);
                    }
                  }}
                >
                  <div className="flex items-center justify-between">
                    <div className="flex items-center gap-2">
                      <span className="font-mono text-xs">{agent.session_id.slice(0, 8)}</span>
                      <Badge variant={assignments.length > 0 ? 'default' : 'muted'}>
                        {assignments.length} app{assignments.length === 1 ? '' : 's'}
                      </Badge>
                    </div>
                    <div className="flex items-center gap-1 text-[11px] text-muted-foreground">
                      <span>{agent.location_hint || '—'}</span>
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={(event) => {
                          event.stopPropagation();
                          onAddToLayout(agent.session_id);
                        }}
                      >
                        Pin
                      </Button>
                    </div>
                  </div>
                  {assignments.length > 0 && (
                    <ul className="mt-2 space-y-1">
                      {assignments.map((edge) => {
                        const status = pairingStatusDisplay(edge.pairing);
                        const application =
                          edge.application ??
                          applicationLookup.get(edge.pairing.child_session_id) ??
                          null;
                        const label = application
                          ? application.session_id.slice(0, 8)
                          : edge.pairing.child_session_id.slice(0, 8);
                        return (
                          <li
                            key={
                              edge.pairing.pairing_id ??
                              `${edge.pairing.controller_session_id}|${edge.pairing.child_session_id}`
                            }
                            className="flex items-center justify-between rounded border border-border/70 bg-muted/40 px-2 py-1 text-[11px]"
                          >
                            <button
                              type="button"
                              className="flex items-center gap-2 text-left text-foreground transition hover:text-primary"
                              onClick={() => onOpenAssignment(edge.pairing)}
                            >
                              <span className="font-mono text-xs">{label}</span>
                              <Badge variant={status.variant}>{status.label}</Badge>
                            </button>
                            <Button
                              size="sm"
                              variant="ghost"
                              onClick={(event) => {
                                event.stopPropagation();
                                onRemoveAssignment(agent.session_id, edge.pairing.child_session_id);
                              }}
                            >
                              ✕
                            </Button>
                          </li>
                        );
                      })}
                    </ul>
                  )}
                </li>
              );
            })}
          </ul>
        </section>
        <section>
          <div className="mb-2 flex items-center justify-between text-xs uppercase tracking-wide text-muted-foreground">
            <span>Applications</span>
            <span>{applications.length}</span>
          </div>
          <ul className="space-y-2">
            {applications.length === 0 && (
              <li className="rounded border border-dashed border-border/70 px-3 py-2 text-[11px] text-muted-foreground">
                No applications yet. Attach sessions or convert agents back to applications.
              </li>
            )}
            {applications.map((application) => {
              const assignments = assignmentsByApplication.get(application.session_id) ?? [];
              const isSelected = selectedApplicationId === application.session_id;
              const assignedAgentIds = assignments.map((pairing) => pairing.controller_session_id);
              const availableAgents = agents.filter(
                (agent) => !assignedAgentIds.includes(agent.session_id),
              );
              const currentSelectValue = assignSelections[application.session_id] ?? '';
              return (
                <li
                  key={application.session_id}
                  className={`rounded border px-3 py-2 text-sm transition ${
                    isSelected ? 'border-primary bg-primary/5' : 'border-border hover:border-primary/60'
                  }`}
                  draggable
                  onDragStart={(event) => {
                    if (!event.dataTransfer) return;
                    event.dataTransfer.effectAllowed = 'copy';
                    event.dataTransfer.setData(APPLICATION_DRAG_TYPE, application.session_id);
                    event.dataTransfer.setData('text/plain', application.session_id);
                  }}
                  onClick={() =>
                    onSelectApplication(isSelected ? null : application.session_id)
                  }
                >
                  <div className="flex items-center justify-between">
                    <div className="flex items-center gap-2">
                      <span className="font-mono text-xs">{application.session_id.slice(0, 8)}</span>
                      <Badge variant={assignments.length > 0 ? 'default' : 'muted'}>
                        {assignments.length} controller{assignments.length === 1 ? '' : 's'}
                      </Badge>
                    </div>
                    <div className="flex items-center gap-1 text-[11px] text-muted-foreground">
                      <span>{application.location_hint || '—'}</span>
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={(event) => {
                          event.stopPropagation();
                          onAddToLayout(application.session_id);
                        }}
                      >
                        Pin
                      </Button>
                    </div>
                  </div>
                  <div className="mt-2 flex items-center justify-between gap-2">
                    <Select
                      value={currentSelectValue}
                      onChange={(value) => {
                        handleAssignSelection(application.session_id, value);
                      }}
                      options={[
                        { value: '', label: availableAgents.length === 0 ? 'No agents available' : 'Assign…' },
                        ...availableAgents.map((agent) => ({
                          value: agent.session_id,
                          label: `Agent ${agent.session_id.slice(0, 8)}`,
                        })),
                      ]}
                      className="w-full text-xs"
                      disabled={availableAgents.length === 0}
                    />
                  </div>
                  {assignments.length > 0 && (
                    <div className="mt-2 flex flex-wrap gap-1 text-[11px] text-muted-foreground">
                      {assignments.map((pairing) => (
                        <Badge
                          key={`${pairing.controller_session_id}|${pairing.child_session_id}`}
                          variant="muted"
                        >
                          Agent {pairing.controller_session_id.slice(0, 8)}
                        </Badge>
                      ))}
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        </section>
      </div>
    </div>
  );
}
