import type { ControllerPairing, SessionRole, SessionSummary } from './api';
import { deriveSessionRole } from './api';

export type AssignmentEdge = {
  pairing: ControllerPairing;
  application: SessionSummary | null;
};

export type AgentAssignments = {
  agent: SessionSummary;
  assignments: AssignmentEdge[];
};

export type ApplicationAssignments = {
  application: SessionSummary;
  controllers: ControllerPairing[];
};

export type AssignmentModel = {
  roles: Map<string, SessionRole>;
  agents: SessionSummary[];
  applications: SessionSummary[];
  assignmentsByAgent: Map<string, AssignmentEdge[]>;
  assignmentsByApplication: Map<string, ControllerPairing[]>;
};

export function buildAssignmentModel(
  sessions: SessionSummary[],
  assignments: ControllerPairing[],
): AssignmentModel {
  const sessionById = new Map<string, SessionSummary>();
  sessions.forEach((session) => {
    sessionById.set(session.session_id, session);
  });

  const roles = new Map<string, SessionRole>();

  const assignmentsByAgent = new Map<string, AssignmentEdge[]>();
  const assignmentsByApplication = new Map<string, ControllerPairing[]>();

  assignments.forEach((pairing) => {
    const controllerId = pairing.controller_session_id;
    const childId = pairing.child_session_id;
    const agentAssignments = assignmentsByAgent.get(controllerId) ?? [];
    const applicationSession = sessionById.get(childId) ?? null;
    agentAssignments.push({
      pairing,
      application: applicationSession,
    });
    assignmentsByAgent.set(controllerId, agentAssignments);

    const appAssignments = assignmentsByApplication.get(childId) ?? [];
    appAssignments.push(pairing);
    assignmentsByApplication.set(childId, appAssignments);
  });

  sessions.forEach((session) => {
    const role = deriveSessionRole(session, assignmentsByAgent.get(session.session_id)?.map((edge) => edge.pairing) ?? assignments);
    roles.set(session.session_id, role);
  });

  const agents: SessionSummary[] = [];
  const applications: SessionSummary[] = [];

  sessions.forEach((session) => {
    const role = roles.get(session.session_id);
    if (role === 'agent') {
      agents.push(session);
    } else {
      applications.push(session);
    }
  });

  agents.sort((a, b) => a.session_id.localeCompare(b.session_id));
  applications.sort((a, b) => a.session_id.localeCompare(b.session_id));

  // Ensure we have empty arrays for agents with no assignments.
  agents.forEach((agent) => {
    if (!assignmentsByAgent.has(agent.session_id)) {
      assignmentsByAgent.set(agent.session_id, []);
    }
  });

  return {
    roles,
    agents,
    applications,
    assignmentsByAgent,
    assignmentsByApplication,
  };
}
