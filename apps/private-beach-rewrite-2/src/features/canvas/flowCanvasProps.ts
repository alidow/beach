export type ReactFlowCanvasProps = {
  nodeDragHandle: string;
  onlyRenderVisibleElements: boolean;
  selectNodesOnDrag: boolean;
  elevateNodesOnSelect: boolean;
  nodesDraggable: boolean;
  zoomOnScroll: boolean;
};

export function computeReactFlowProps(isInteractive: boolean): ReactFlowCanvasProps {
  return {
    nodeDragHandle: '.rf-drag-handle',
    onlyRenderVisibleElements: false,
    selectNodesOnDrag: false,
    elevateNodesOnSelect: false,
    nodesDraggable: true,
    zoomOnScroll: !isInteractive,
  };
}
