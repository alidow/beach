import { describe, expect, it } from 'vitest';

import {
  addTileToGroup,
  createGroupWithTiles,
  dropTileOnTarget,
  removeTileFromGroup,
} from '../layoutOps';
import type { CanvasLayout } from '../types';

function makeBaseLayout(): CanvasLayout {
  const now = Date.now();
  return {
    version: 3,
    viewport: { zoom: 1, pan: { x: 0, y: 0 } },
    tiles: {
      a: {
        id: 'a',
        kind: 'application',
        position: { x: 0, y: 0 },
        size: { width: 200, height: 120 },
        zIndex: 1,
      },
      b: {
        id: 'b',
        kind: 'application',
        position: { x: 240, y: 0 },
        size: { width: 200, height: 120 },
        zIndex: 2,
      },
      c: {
        id: 'c',
        kind: 'application',
        position: { x: 480, y: 0 },
        size: { width: 200, height: 120 },
        zIndex: 3,
      },
    },
    groups: {},
    agents: {},
    controlAssignments: {},
    metadata: { createdAt: now, updatedAt: now },
  };
}

describe('layoutOps grouping', () => {
  it('preserves group metadata through add/remove and JSON round-trip', () => {
    const base = makeBaseLayout();
    const withGroup = createGroupWithTiles(base, 'a', 'b');
    const groupId = Object.keys(withGroup.groups)[0];
    expect(groupId).toBeDefined();
    const group = withGroup.groups[groupId!];
    expect(group.name).toBe('Group 1');
    expect(group.padding).toBe(16);
    expect(group.memberIds.sort()).toEqual(['a', 'b']);
    expect(withGroup.tiles.a.groupId).toBe(groupId);
    expect(withGroup.tiles.b.groupId).toBe(groupId);

    const added = addTileToGroup(withGroup, 'c', groupId!);
    const updatedGroup = added.groups[groupId!];
    expect(updatedGroup.memberIds.sort()).toEqual(['a', 'b', 'c']);
    expect(updatedGroup.padding).toBe(16);
    expect(updatedGroup.name).toBe('Group 1');

    const jsonRoundTrip = JSON.parse(JSON.stringify(added)) as CanvasLayout;
    expect(jsonRoundTrip.groups[groupId!].padding).toBe(16);

    const removed = removeTileFromGroup(added, 'c', groupId!);
    expect(removed.groups[groupId!].memberIds.sort()).toEqual(['a', 'b']);

    const dissolved = removeTileFromGroup(removed, 'b', groupId!);
    expect(dissolved.groups[groupId!]).toBeUndefined();
    expect(dissolved.tiles.a.groupId).toBeUndefined();
    expect(dissolved.tiles.b.groupId).toBeUndefined();
  });

  it('ignores self-drop attempts that would otherwise create a group', () => {
    const base = makeBaseLayout();
    const result = dropTileOnTarget(base, 'a', { type: 'tile', id: 'a' });
    expect(result).toBe(base);
    expect(Object.keys(result.groups)).toHaveLength(0);
  });
});
