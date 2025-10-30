import { describe, expect, it } from 'vitest';

import { getTileBorderClass } from '../CanvasSurface';

describe('getTileBorderClass', () => {
  it('does not leave the drag glow when tile is only selected', () => {
    const result = getTileBorderClass({ selected: true, isDropTarget: false });
    expect(result).not.toMatch(/shadow-\[/);
  });

  it('preserves the drag glow while the tile is actively dragging', () => {
    const result = getTileBorderClass({ selected: false, isDropTarget: false, isDragging: true });
    expect(result).toMatch(/shadow-\[/);
  });
});
