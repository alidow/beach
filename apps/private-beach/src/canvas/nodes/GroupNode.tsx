import { memo } from 'react';

type Props = {
  width: number;
  height: number;
  name?: string;
  padding?: number;
  selected?: boolean;
  members?: { id: string; x: number; y: number; w: number; h: number }[];
};

// Minimal, dependency-free visual for group nodes. React Flow integration is handled in CanvasSurface.
export const GroupNode = memo(function GroupNode({ width, height, name, padding = 16, selected, members = [] }: Props) {
  return (
    <div
      role="group"
      aria-label={name || 'Group'}
      className={
        'rounded-xl border bg-card/50 shadow-sm ' +
        (selected ? 'border-primary/60 ring-2 ring-primary/20' : 'border-border')
      }
      style={{ width, height, padding }}
      data-testid="canvas-group-node"
    >
      <div className="flex items-center justify-between pb-2 text-xs text-muted-foreground">
        <div className="truncate font-medium">{name || 'Group'}</div>
        <div className="opacity-70">{members.length} {members.length === 1 ? 'item' : 'items'}</div>
      </div>
      <div className="relative h-full w-full">
        {members.map((m) => (
          <div
            key={m.id}
            className="absolute rounded-md bg-muted/30"
            style={{ left: m.x, top: m.y, width: m.w, height: m.h }}
            aria-hidden={true}
          />
        ))}
      </div>
    </div>
  );
});

