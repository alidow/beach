import type {
  CanvasLayout,
  CanvasGroupNode,
  CanvasTileNode,
  CanvasPoint,
  DropTarget,
} from './types';

const DEFAULT_GROUP_PADDING = 16;

export function withUpdatedTimestamp(layout: CanvasLayout): CanvasLayout {
  return { ...layout, metadata: { ...layout.metadata, updatedAt: Date.now() } };
}

export function boundsOfTile(tile: CanvasTileNode) {
  return {
    left: tile.position.x,
    top: tile.position.y,
    right: tile.position.x + tile.size.width,
    bottom: tile.position.y + tile.size.height,
  };
}

export function unionBounds(bounds: { left: number; top: number; right: number; bottom: number }[]) {
  if (bounds.length === 0) {
    return { left: 0, top: 0, right: 0, bottom: 0 };
  }
  let left = bounds[0].left;
  let top = bounds[0].top;
  let right = bounds[0].right;
  let bottom = bounds[0].bottom;
  for (let i = 1; i < bounds.length; i++) {
    const b = bounds[i];
    if (b.left < left) left = b.left;
    if (b.top < top) top = b.top;
    if (b.right > right) right = b.right;
    if (b.bottom > bottom) bottom = b.bottom;
  }
  return { left, top, right, bottom };
}

export function computeGroupFromMembers(
  layout: CanvasLayout,
  memberIds: string[],
  padding = DEFAULT_GROUP_PADDING,
): Pick<CanvasGroupNode, 'position' | 'size' | 'memberIds' | 'padding'> {
  const tiles = memberIds
    .map((id) => layout.tiles[id])
    .filter((t): t is CanvasTileNode => Boolean(t));
  const u = unionBounds(tiles.map(boundsOfTile));
  return {
    memberIds: [...new Set(memberIds)],
    position: { x: u.left - padding, y: u.top - padding },
    size: { width: u.right - u.left + padding * 2, height: u.bottom - u.top + padding * 2 },
    padding,
  };
}

export function createGroupWithTiles(
  layout: CanvasLayout,
  tileAId: string,
  tileBId: string,
  name?: string,
): CanvasLayout {
  if (!layout.tiles[tileAId] || !layout.tiles[tileBId]) return layout;
  const id = `group_${tileAId}_${tileBId}_${Date.now()}`;
  const { position, size, memberIds, padding } = computeGroupFromMembers(layout, [tileAId, tileBId]);
  const zIndex = Math.max(0, ...Object.values(layout.groups).map((g) => g.zIndex), ...Object.values(layout.tiles).map((t) => t.zIndex)) + 1;
  const label = name && name.trim().length > 0 ? name.trim() : `Group ${Object.keys(layout.groups).length + 1}`;
  const group: CanvasGroupNode = {
    id,
    name: label,
    memberIds,
    position,
    size,
    zIndex,
    padding,
  };
  const next: CanvasLayout = {
    ...layout,
    groups: { ...layout.groups, [id]: group },
    tiles: {
      ...layout.tiles,
      [tileAId]: { ...layout.tiles[tileAId], groupId: id },
      [tileBId]: { ...layout.tiles[tileBId], groupId: id },
    },
  };
  return withUpdatedTimestamp(next);
}

export function addTileToGroup(layout: CanvasLayout, tileId: string, groupId: string): CanvasLayout {
  const tile = layout.tiles[tileId];
  const group = layout.groups[groupId];
  if (!tile || !group) return layout;
  if (group.memberIds.includes(tileId)) return layout;
  const memberIds = [...group.memberIds, tileId];
  const { position, size } = computeGroupFromMembers(layout, memberIds, group.padding ?? DEFAULT_GROUP_PADDING);
  const next: CanvasLayout = {
    ...layout,
    groups: { ...layout.groups, [groupId]: { ...group, memberIds, position, size } },
    tiles: { ...layout.tiles, [tileId]: { ...tile, groupId } },
  };
  return withUpdatedTimestamp(next);
}

export function removeTileFromGroup(layout: CanvasLayout, tileId: string, groupId: string): CanvasLayout {
  const tile = layout.tiles[tileId];
  const group = layout.groups[groupId];
  if (!tile || !group) return layout;
  const memberIds = group.memberIds.filter((id) => id !== tileId);
  const tiles: CanvasLayout['tiles'] = {
    ...layout.tiles,
    [tileId]: { ...tile, groupId: undefined },
  };
  if (memberIds.length <= 1) {
    for (const memberId of group.memberIds) {
      if (memberId === tileId) continue;
      const member = layout.tiles[memberId];
      if (member && member.groupId === groupId) {
        tiles[memberId] = { ...member, groupId: undefined };
      }
    }
    // dissolve group
    const { [groupId]: _omit, ...rest } = layout.groups;
    const next: CanvasLayout = { ...layout, groups: rest, tiles };
    return withUpdatedTimestamp(next);
  }
  const { position, size } = computeGroupFromMembers(layout, memberIds, group.padding ?? DEFAULT_GROUP_PADDING);
  const next: CanvasLayout = {
    ...layout,
    groups: { ...layout.groups, [groupId]: { ...group, memberIds, position, size } },
    tiles,
  };
  return withUpdatedTimestamp(next);
}

export function recomputeGroupBox(layout: CanvasLayout, groupId: string): CanvasLayout {
  const group = layout.groups[groupId];
  if (!group) return layout;
  const { position, size } = computeGroupFromMembers(layout, group.memberIds, group.padding ?? DEFAULT_GROUP_PADDING);
  const next: CanvasLayout = {
    ...layout,
    groups: { ...layout.groups, [groupId]: { ...group, position, size } },
  };
  return withUpdatedTimestamp(next);
}

export function hitTest(layout: CanvasLayout, point: CanvasPoint): DropTarget {
  // Prefer agents, then groups, then tiles for drop priorities
  for (const agent of Object.values(layout.agents)) {
    const { x, y } = agent.position;
    const { width, height } = agent.size;
    if (point.x >= x && point.x <= x + width && point.y >= y && point.y <= y + height) {
      return { type: 'agent', id: agent.id };
    }
  }
  for (const group of Object.values(layout.groups)) {
    const { x, y } = group.position;
    const { width, height } = group.size;
    if (point.x >= x && point.x <= x + width && point.y >= y && point.y <= y + height) {
      return { type: 'group', id: group.id };
    }
  }
  for (const tile of Object.values(layout.tiles)) {
    const { x, y } = tile.position;
    const { width, height } = tile.size;
    if (point.x >= x && point.x <= x + width && point.y >= y && point.y <= y + height) {
      return { type: 'tile', id: tile.id };
    }
  }
  return { type: 'none' };
}

export function dropTileOnTarget(layout: CanvasLayout, tileId: string, target: DropTarget): CanvasLayout {
  if (target.type === 'tile') {
    if (target.id === tileId) {
      return layout;
    }
    // Create a new group containing both tiles (or merge if target is already grouped)
    const dst = layout.tiles[target.id];
    if (!dst) return layout;
    if (dst.groupId) {
      return addTileToGroup(layout, tileId, dst.groupId);
    }
    return createGroupWithTiles(layout, tileId, target.id);
  }
  if (target.type === 'group') {
    return addTileToGroup(layout, tileId, target.id);
  }
  // Agent handled by assignment flow outside this reducer
  return layout;
}
