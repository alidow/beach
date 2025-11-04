'use client';

import { useDraggable } from '@dnd-kit/core';
import { useMemo } from 'react';
import { useCanvasUI } from './CanvasContext';
import { NodeCardContent } from './NodeCard';
import type { CanvasNodeDefinition } from './types';

type NodeDrawerProps = {
  nodes: CanvasNodeDefinition[];
  activeNodeId: string | null;
};

const CARD_BASE_CLASSES =
  'relative select-none rounded-lg border border-border bg-card/90 p-4 shadow-sm transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary';
const CARD_HOVER_CLASSES = 'hover:border-primary/70 hover:bg-card';
const CARD_ACTIVE_CLASSES = 'ring-2 ring-primary border-transparent';
const CARD_DRAGGING_CLASSES = 'cursor-grabbing opacity-70';

function buildCardClassName({ active, dragging }: { active: boolean; dragging: boolean }) {
  return [
    CARD_BASE_CLASSES,
    CARD_HOVER_CLASSES,
    active ? CARD_ACTIVE_CLASSES : '',
    dragging ? CARD_DRAGGING_CLASSES : 'cursor-grab',
  ]
    .filter(Boolean)
    .join(' ');
}

type DraggableNodeCardProps = {
  node: CanvasNodeDefinition;
  isActive: boolean;
};

function DraggableNodeCard({ node, isActive }: DraggableNodeCardProps) {
  const { attributes, listeners, setNodeRef, transform, isDragging } = useDraggable({
    id: `catalog-${node.id}`,
    data: { catalogNode: node },
  });

  const style = useMemo(
    () =>
      transform
        ? {
            transform: `translate3d(${transform.x}px, ${transform.y}px, 0)`,
          }
        : undefined,
    [transform],
  );

  const className = buildCardClassName({ active: isActive || isDragging, dragging: isDragging });

  return (
    <div
      ref={setNodeRef}
      style={style}
      className={className}
      tabIndex={0}
      {...listeners}
      {...attributes}
    >
      <NodeCardContent node={node} />
    </div>
  );
}

export function NodeDrawer({ nodes, activeNodeId }: NodeDrawerProps) {
  const { drawerOpen, toggleDrawer } = useCanvasUI();

  const asideClass = [
    'relative flex h-full flex-col border-l border-border bg-secondary/40 backdrop-blur-sm transition-[width] duration-150 ease-in-out',
    drawerOpen ? 'w-80' : 'w-12',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <aside className={asideClass} aria-label="Node catalog">
      <div className="flex items-center justify-end px-2 py-3">
        <button
          type="button"
          onClick={toggleDrawer}
          className="inline-flex h-8 w-8 items-center justify-center rounded-full border border-border bg-card text-sm font-medium text-foreground shadow-sm transition hover:bg-card/80 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
          aria-expanded={drawerOpen}
          aria-label={drawerOpen ? 'Collapse node drawer' : 'Expand node drawer'}
        >
          {drawerOpen ? '>>' : '<<'}
        </button>
      </div>
      {drawerOpen ? (
        <div className="flex flex-col gap-4 px-4 pb-4">
          <header className="space-y-1">
            <h2 className="text-sm font-semibold text-foreground">Node Catalog</h2>
            <p className="text-xs text-muted-foreground">
              Drag nodes onto the canvas to create new tiles. Drops snap to the 8px grid.
            </p>
          </header>
          <div className="flex-1 space-y-3 overflow-y-auto pr-1">
            {nodes.length === 0 ? (
              <p className="rounded-md border border-dashed border-border bg-background/60 p-3 text-xs text-muted-foreground">
                No nodes registered yet. Coordinate with WS-A to populate catalog data.
              </p>
            ) : (
              nodes.map((node) => (
                <DraggableNodeCard key={node.id} node={node} isActive={activeNodeId === node.id} />
              ))
            )}
          </div>
        </div>
      ) : (
        <div className="flex flex-1 items-center justify-center">
          <span className="-rotate-90 text-xs font-semibold uppercase tracking-widest text-muted-foreground">
            Catalog
          </span>
        </div>
      )}
    </aside>
  );
}

export function NodeCatalogPreview({ node }: { node: CanvasNodeDefinition }) {
  return (
    <div className={buildCardClassName({ active: true, dragging: true })}>
      <NodeCardContent node={node} />
    </div>
  );
}
