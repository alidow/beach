import type { CanvasLayout, DropTarget } from './types';
import { onDropTile, onDragOver } from './dragDropOps';
import {
  applyOptimisticAssignments,
  createAssignmentsBatch,
  type BatchAssignmentItem,
  type PendingAssignment,
} from './assignments';

export type CanvasSurfaceAdapter = {
  toCanvasPoint: (clientX: number, clientY: number) => { x: number; y: number };
};

export type DropSource = { type: 'tile'; id: string } | { type: 'group'; id: string };

export function previewDropTarget(
  layout: CanvasLayout,
  adapter: CanvasSurfaceAdapter,
  clientX: number,
  clientY: number,
) {
  return onDragOver(layout, adapter, clientX, clientY);
}

export function applyDrop(
  layout: CanvasLayout,
  source: DropSource,
  target: DropTarget,
): { layout: CanvasLayout; pendingAssignment?: PendingAssignment } {
  if (source.type === 'tile') {
    if (target.type === 'agent') {
      const next = applyOptimisticAssignments(layout, target.id, { type: 'tile', id: source.id });
      return {
        layout: next,
        pendingAssignment: { controllerId: target.id, target: { type: 'tile', id: source.id } },
      };
    }
    return { layout: onDropTile(layout, source.id, target) };
  }
  if (source.type === 'group') {
    if (target.type === 'agent') {
      const next = applyOptimisticAssignments(layout, target.id, { type: 'group', id: source.id });
      return {
        layout: next,
        pendingAssignment: { controllerId: target.id, target: { type: 'group', id: source.id } },
      };
    }
    // dropping group on tile/group is a no-op for now
    return { layout };
  }
  return { layout };
}

export async function fulfillPendingAssignment(
  layout: CanvasLayout,
  pending: PendingAssignment,
  managerToken: string,
  managerUrl?: string,
  opts?: { privateBeachId?: string },
) {
  const items: BatchAssignmentItem[] = [];
  if (pending.target.type === 'tile') {
    items.push({ controllerId: pending.controllerId, childIds: [pending.target.id] });
  } else {
    const group = layout.groups[pending.target.id];
    const memberIds = group?.memberIds ?? [];
    items.push({ controllerId: pending.controllerId, childIds: memberIds });
  }
  return await createAssignmentsBatch(items, managerToken, managerUrl, {
    privateBeachId: opts?.privateBeachId,
  });
}

export type { PendingAssignment, PendingAssignmentTarget } from './assignments';
