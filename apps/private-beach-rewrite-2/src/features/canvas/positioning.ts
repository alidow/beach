import type { CanvasBounds, CanvasPoint } from './types';

export function snapPointToGrid(point: CanvasPoint, gridSize: number): CanvasPoint {
  if (!Number.isFinite(gridSize) || gridSize <= 0) {
    return point;
  }
  const snap = (value: number) => {
    if (!Number.isFinite(value)) {
      return 0;
    }
    return Math.round(value / gridSize) * gridSize;
  };
  return { x: snap(point.x), y: snap(point.y) };
}

export function clampPointToBounds(
  position: CanvasPoint,
  size: { width: number; height: number },
  bounds: CanvasBounds,
): CanvasPoint {
  const limitX = Math.max(0, bounds.width - size.width);
  const limitY = Math.max(0, bounds.height - size.height);
  const normalize = (value: number) => (Number.isFinite(value) ? value : 0);
  const x = Math.min(Math.max(normalize(position.x), 0), limitX);
  const y = Math.min(Math.max(normalize(position.y), 0), limitY);
  return { x, y };
}
