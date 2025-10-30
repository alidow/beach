import type { CanvasLayout, DropTarget } from './types';
import { dropTileOnTarget, hitTest } from './layoutOps';

export type DragContext = {
  // Screen → canvas transform helpers can be injected by CanvasSurface
  toCanvasPoint: (clientX: number, clientY: number) => { x: number; y: number };
};

export function onDragOver(
  layout: CanvasLayout,
  ctx: DragContext,
  clientX: number,
  clientY: number,
): DropTarget {
  const p = ctx.toCanvasPoint(clientX, clientY);
  return hitTest(layout, p);
}

export function onDropTile(
  layout: CanvasLayout,
  tileId: string,
  dropTarget: DropTarget,
): CanvasLayout {
  return dropTileOnTarget(layout, tileId, dropTarget);
}

