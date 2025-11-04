export type CanvasNodeDefinition = {
  id: string;
  nodeType: string;
  label: string;
  description?: string;
  defaultSize: {
    width: number;
    height: number;
  };
};

export type CanvasBounds = {
  width: number;
  height: number;
};

export type CanvasPoint = {
  x: number;
  y: number;
};

export type NodePlacementPayload = {
  catalogId: string;
  nodeType: string;
  size: {
    width: number;
    height: number;
  };
  /**
   * Raw pointer-aligned coordinates captured from the drag event before snapping.
   */
  rawPosition: CanvasPoint;
  /**
   * Snapped and clamped coordinates suitable for positioning a tile on the canvas grid.
   */
  snappedPosition: CanvasPoint;
  canvasBounds: CanvasBounds;
  gridSize: number;
  source: 'catalog';
};

export type TileMovePayload = {
  tileId: string;
  source: 'pointer' | 'keyboard';
  rawPosition: CanvasPoint;
  snappedPosition: CanvasPoint;
  delta: CanvasPoint;
  canvasBounds: CanvasBounds;
  gridSize: number;
  timestamp: number;
};
