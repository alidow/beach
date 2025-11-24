import { describe, expect, it } from 'vitest';
import { computeReactFlowProps } from '../flowCanvasProps';

describe('FlowCanvas ReactFlow props', () => {
  it('sets anti-flicker and drag-related props', () => {
    const props = computeReactFlowProps(false);
    expect(props.nodeDragHandle).toBe('.rf-drag-handle');
    expect(props.onlyRenderVisibleElements).toBe(false);
    expect(props.selectNodesOnDrag).toBe(false);
    expect(props.elevateNodesOnSelect).toBe(false);
    expect(props.nodesDraggable).toBe(true);
    expect(props.zoomOnScroll).toBe(true);
  });

  it('disables zoom-on-scroll when a tile is interactive', () => {
    const props = computeReactFlowProps(true);
    expect(props.zoomOnScroll).toBe(false);
  });
});
