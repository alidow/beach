import type { CanvasLayout } from './types';
import { createGroupWithTiles, removeTileFromGroup, addTileToGroup } from './layoutOps';

export function groupSelection(layout: CanvasLayout, selectedTileIds: string[]): CanvasLayout {
  if (selectedTileIds.length < 2) return layout;
  // Start by grouping the first two, then add the rest
  let next = createGroupWithTiles(layout, selectedTileIds[0], selectedTileIds[1]);
  const groupId = Object.values(next.groups).find((g) => g.memberIds.includes(selectedTileIds[0]) && g.memberIds.includes(selectedTileIds[1]))?.id;
  if (!groupId) return next;
  for (let i = 2; i < selectedTileIds.length; i++) {
    const tileId = selectedTileIds[i];
    next = addTileToGroup(next, tileId, groupId);
  }
  return next;
}

export function ungroupSelection(layout: CanvasLayout, selectedTileIds: string[]): CanvasLayout {
  let next = layout;
  for (const tileId of selectedTileIds) {
    const tile = next.tiles[tileId];
    if (!tile?.groupId) continue;
    next = removeTileFromGroup(next, tileId, tile.groupId);
  }
  return next;
}
