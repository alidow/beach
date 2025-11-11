'use client';

import { useCallback, useMemo, useState } from 'react';
import type { DragEvent } from 'react';
import { Boxes } from 'lucide-react';
import { useCanvasUI } from './CanvasContext';
import { NodeCardContent } from './NodeCard';
import type { CanvasNodeDefinition } from './types';

type NodeDrawerProps = {
  nodes: CanvasNodeDefinition[];
  activeNodeId: string | null;
  onNodeDragStart?: (nodeId: string) => void;
  onNodeDragEnd?: () => void;
};

const CARD_BASE_CLASSES =
  'relative select-none rounded-2xl border border-white/10 bg-slate-950/60 p-4 shadow-[0_20px_60px_rgba(2,6,23,0.55)] transition-colors duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-offset-0 focus-visible:ring-sky-400/60';
const CARD_HOVER_CLASSES = 'hover:border-sky-400/60 hover:bg-slate-900/70';
const CARD_ACTIVE_CLASSES = 'ring-2 ring-sky-400/70 border-sky-400/70 bg-slate-900/60';
const CARD_DRAGGING_CLASSES = 'cursor-grabbing opacity-70';

const APPLICATION_MIME = 'application/reactflow';
const OFFSET_MIME = 'application/reactflow-offset';

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
  disabled?: boolean;
  onDragStart?: (nodeId: string) => void;
  onDragEnd?: () => void;
};

function DraggableNodeCard({ node, isActive, disabled = false, onDragStart, onDragEnd }: DraggableNodeCardProps) {
  const [dragging, setDragging] = useState(false);

  const handleDragStart = useCallback(
    (event: DragEvent<HTMLDivElement>) => {
      if (disabled) {
        event.preventDefault();
        return;
      }
      setDragging(true);
      console.info('[ws-c][catalog] drag-start', { nodeId: node.id, label: node.label });
      onDragStart?.(node.id);

      const dataTransfer = event.dataTransfer;
      if (dataTransfer) {
        dataTransfer.effectAllowed = 'copy';
        dataTransfer.setData(
          APPLICATION_MIME,
          JSON.stringify({
            id: node.id,
            nodeType: node.nodeType,
            defaultSize: node.defaultSize,
          }),
        );
        const rect = event.currentTarget.getBoundingClientRect();
        const offset = {
          x: event.clientX - rect.left,
          y: event.clientY - rect.top,
        };
        console.info('[ws-c][catalog] drag-offset', { nodeId: node.id, offset });
        dataTransfer.setData(OFFSET_MIME, JSON.stringify(offset));
        try {
          dataTransfer.setDragImage(event.currentTarget, offset.x, offset.y);
        } catch (error) {
          console.warn('[ws-c][catalog] drag-image-failed', { error });
          // Ignore drag image issues (e.g., Firefox).
        }
      }
    },
    [disabled, node.defaultSize, node.id, node.label, node.nodeType, onDragStart],
  );

  const handleDragEnd = useCallback(() => {
    if (disabled) {
      return;
    }
    setDragging(false);
    console.info('[ws-c][catalog] drag-end', { nodeId: node.id });
    onDragEnd?.();
  }, [disabled, node.id, onDragEnd]);

  const className = useMemo(
    () => buildCardClassName({ active: isActive || dragging, dragging }),
    [dragging, isActive],
  );

  return (
    <div
      draggable={!disabled}
      onDragStart={handleDragStart}
      onDragEnd={handleDragEnd}
      className={className}
      tabIndex={disabled ? -1 : 0}
      aria-roledescription="Draggable node"
      data-testid={`catalog-node-${node.id}`}
    >
      <NodeCardContent node={node} />
    </div>
  );
}

export function NodeDrawer({ nodes, activeNodeId, onNodeDragStart, onNodeDragEnd }: NodeDrawerProps) {
  const { drawerOpen, openDrawer, closeDrawer } = useCanvasUI();

  const panelClass = [
    'absolute left-6 top-16 z-40 w-80 max-w-[90vw] rounded-[28px] border border-white/10 bg-slate-950/80 p-5 shadow-[0_35px_120px_rgba(2,6,23,0.7)] backdrop-blur-xl transition-all duration-200 ease-out',
    drawerOpen ? 'pointer-events-auto translate-y-0 opacity-100' : 'pointer-events-none -translate-y-3 opacity-0',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <>
      {!drawerOpen ? (
        <button
          type="button"
          onClick={openDrawer}
          className="pointer-events-auto absolute left-6 top-3 z-50 inline-flex h-10 w-10 items-center justify-center rounded-full border border-white/10 bg-slate-950/70 text-slate-200 shadow-[0_15px_40px_rgba(2,6,23,0.65)] transition hover:border-white/30 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60"
          aria-expanded={false}
          aria-label="Show node catalog"
        >
          <Boxes className="h-4 w-4" aria-hidden="true" />
        </button>
      ) : null}
      <aside className={panelClass} aria-label="Node catalog" aria-hidden={!drawerOpen} data-state={drawerOpen ? 'open' : 'closed'}>
        <div className="mb-4 flex items-center justify-between gap-3">
          <header className="space-y-1">
            <h2 className="text-sm font-semibold text-white">Node Catalog</h2>
            <p className="text-xs text-slate-400">
              Drag onto the surface. Hold <kbd className="rounded bg-white/10 px-1 text-[10px] font-semibold">Cmd</kbd>+<kbd className="rounded bg-white/10 px-1 text-[10px] font-semibold">B</kbd> to toggle.
            </p>
          </header>
          <button
            type="button"
            onClick={closeDrawer}
            className="inline-flex h-7 items-center justify-center rounded-full border border-white/10 bg-white/5 px-3 text-[11px] font-semibold uppercase tracking-[0.22em] text-slate-300 transition hover:border-white/30 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60"
          >
            Close
          </button>
        </div>
        <div className="flex-1 space-y-3 overflow-y-auto pr-1">
          {nodes.length === 0 ? (
            <p className="rounded-xl border border-dashed border-white/10 bg-white/5 p-3 text-xs text-slate-400">
              No nodes registered yet. Coordinate with WS-A to populate catalog data.
            </p>
          ) : (
            nodes.map((node) => (
              <DraggableNodeCard
                key={node.id}
                node={node}
                isActive={activeNodeId === node.id}
                disabled={!drawerOpen}
                onDragStart={onNodeDragStart}
                onDragEnd={onNodeDragEnd}
              />
            ))
          )}
        </div>
      </aside>
    </>
  );
}
