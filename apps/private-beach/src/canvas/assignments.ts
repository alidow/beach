import type { CanvasLayout } from './types';
import { withUpdatedTimestamp } from './layoutOps';
import {
  batchControllerAssignments,
  createControllerPairing,
  type ControllerAssignmentResult,
  type ControllerPairing,
  type ControllerUpdateCadence,
} from '../lib/api';

export type BatchAssignmentItem = {
  controllerId: string; // agent id
  childIds: string[]; // application session ids
  promptTemplate?: string | null;
  updateCadence?: ControllerUpdateCadence;
};

export type BatchAssignmentResult = {
  controllerId: string;
  childId: string;
  ok: boolean;
  error?: string;
  pairing?: ControllerPairing;
};

export type BatchAssignmentResponse = {
  results: BatchAssignmentResult[];
};

export type PendingAssignmentTarget = { type: 'tile' | 'group'; id: string };
export type PendingAssignment = { controllerId: string; target: PendingAssignmentTarget };

function assignmentKey(controllerId: string, target: PendingAssignmentTarget) {
  return `${controllerId}|${target.type}|${target.id}`;
}

// Hook for a future backend batch endpoint. If unavailable, falls back to per-child create.
export async function createAssignmentsBatch(
  items: BatchAssignmentItem[],
  managerToken: string,
  managerUrl?: string,
  // optionally provide a custom batch endpoint path if backend exposes one during integration
  opts?: { batchPath?: string; privateBeachId?: string },
): Promise<BatchAssignmentResponse> {
  const results: BatchAssignmentResult[] = [];
  if (opts?.privateBeachId) {
    try {
      const assignmentsPayload = items.flatMap((item) =>
        item.childIds.map((childId) => ({
          controller_session_id: item.controllerId,
          child_session_id: childId,
          prompt_template: item.promptTemplate ?? null,
          update_cadence: item.updateCadence ?? 'balanced',
        })),
      );
      if (assignmentsPayload.length > 0) {
        const response = await batchControllerAssignments(
          opts.privateBeachId,
          assignmentsPayload,
          managerToken,
          managerUrl,
        );
        const mapped = response.map((entry: ControllerAssignmentResult) => ({
          controllerId: entry.controller_session_id,
          childId: entry.child_session_id,
          ok: entry.ok,
          error: entry.error,
          pairing: entry.pairing,
        } satisfies BatchAssignmentResult));
        return { results: mapped };
      }
    } catch (_) {
      // fall through to legacy endpoint / per-session fallback
    }
  }
  const path = opts?.batchPath || '/controller-pairings/batch';
  try {
    const res = await fetch(`${(managerUrl || process.env.NEXT_PUBLIC_MANAGER_URL || 'http://localhost:8080')}${path}`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        authorization: `Bearer ${managerToken}`,
      },
      body: JSON.stringify({
        assignments: items.map((it) => ({
          controller_id: it.controllerId,
          child_session_ids: it.childIds,
          prompt_template: it.promptTemplate ?? null,
          update_cadence: it.updateCadence ?? 'balanced',
        })),
      }),
    });
    if (res.ok) {
      const data = await res.json().catch(() => ({}));
      if (Array.isArray(data?.results)) {
        // Expect backend to return shape aligned with BatchAssignmentResult
        return { results: data.results } as BatchAssignmentResponse;
      }
      // Non-standard but OK — treat as success without details
      for (const item of items) {
        for (const childId of item.childIds) {
          results.push({ controllerId: item.controllerId, childId, ok: true });
        }
      }
      return { results };
    }
    // Intentionally fall through to per-child fallback on non-OK
  } catch {
    // Network or path not found — fall back
  }

  // Fallback to single create per child using existing API
  for (const item of items) {
    for (const childId of item.childIds) {
      try {
        const pairing = await createControllerPairing(
          item.controllerId,
          {
            child_session_id: childId,
            prompt_template: item.promptTemplate ?? null,
            update_cadence: item.updateCadence ?? 'balanced',
          },
          managerToken,
          managerUrl,
        );
        results.push({ controllerId: item.controllerId, childId, ok: true, pairing });
      } catch (err: any) {
        results.push({ controllerId: item.controllerId, childId, ok: false, error: err?.message ?? String(err) });
      }
    }
  }
  return { results };
}

export function applyOptimisticAssignments(
  layout: CanvasLayout,
  controllerId: string,
  target: PendingAssignmentTarget,
): CanvasLayout {
  const key = assignmentKey(controllerId, target);
  const next: CanvasLayout = {
    ...layout,
    controlAssignments: {
      ...layout.controlAssignments,
      [key]: { controllerId, targetType: target.type, targetId: target.id },
    },
  };
  return withUpdatedTimestamp(next);
}

export function removeOptimisticAssignment(
  layout: CanvasLayout,
  controllerId: string,
  target: PendingAssignmentTarget,
): CanvasLayout {
  const key = assignmentKey(controllerId, target);
  if (!layout.controlAssignments[key]) {
    return layout;
  }
  const { [key]: _omit, ...rest } = layout.controlAssignments;
  return withUpdatedTimestamp({ ...layout, controlAssignments: rest });
}

export function applyAssignmentResults(
  layout: CanvasLayout,
  pending: PendingAssignment,
  response: BatchAssignmentResponse,
): CanvasLayout {
  const failures = response.results.filter((r) => !r.ok);
  if (failures.length === 0) {
    // All good — keep optimistic assignment, just update timestamp
    return withUpdatedTimestamp(layout);
  }
  // Partial or full failure: remove optimistic mapping to stay truthful
  return removeOptimisticAssignment(layout, pending.controllerId, pending.target);
}

export function summarizeAssignmentFailures(response: BatchAssignmentResponse): string | null {
  const failures = response.results.filter((r) => !r.ok);
  if (failures.length === 0) return null;
  const details = failures
    .map((r) => `${r.childId}${r.error ? ` (${r.error})` : ''}`)
    .join(', ');
  return failures.length === response.results.length
    ? `Assignment failed${details ? `: ${details}` : '.'}`
    : `Assignment partially failed: ${details}`;
}

export function extractSuccessfulPairings(response: BatchAssignmentResponse): ControllerPairing[] {
  return response.results
    .filter((r): r is BatchAssignmentResult & { pairing: ControllerPairing } => r.ok && Boolean(r.pairing))
    .map((r) => r.pairing);
}
