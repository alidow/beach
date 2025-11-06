'use client';

import { memo, useMemo, useState } from 'react';
import {
  BaseEdge,
  EdgeLabelRenderer,
  getBezierPath,
  type EdgeProps,
} from 'reactflow';
import type { AssignmentEdgeData, UpdateMode } from './types';

const updateModeLabels: Record<UpdateMode, string> = {
  'idle-summary': 'After idle summary',
  push: 'Managed session pushes updates',
  poll: 'Poll on interval',
};

const RelationshipEdge = memo(function RelationshipEdge({
  id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  markerEnd,
  data,
}: EdgeProps<AssignmentEdgeData>) {
  const [edgePath, labelX, labelY] = useMemo(
    () => getBezierPath({ sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition }),
    [sourcePosition, sourceX, sourceY, targetPosition, targetX, targetY],
  );
  const [instructions, setInstructions] = useState(data.instructions);
  const [updateMode, setUpdateMode] = useState<UpdateMode>(data.updateMode);
  const [pollFrequency, setPollFrequency] = useState<number>(data.pollFrequency);

  const isEditing = data.isEditing;

  const onSubmit = (event: React.FormEvent) => {
    event.preventDefault();
    data.onSave({ id, instructions: instructions.trim(), updateMode, pollFrequency });
  };

  return (
    <>
      <BaseEdge path={edgePath} markerEnd={markerEnd} className="stroke-slate-300" />
      <EdgeLabelRenderer>
        <div
          className="pointer-events-auto"
          style={{
            transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)` ,
          }}
        >
          {isEditing ? (
            <form
              onSubmit={onSubmit}
              onPointerDown={(event) => event.stopPropagation()}
              className="w-64 rounded-xl border border-slate-300 bg-white p-3 text-xs shadow-xl"
            >
              <p className="text-[10px] font-semibold uppercase tracking-wider text-slate-500">
                Assignment Instructions
              </p>
              <textarea
                value={instructions}
                onChange={(event) => setInstructions(event.target.value)}
                rows={3}
                className="mt-2 w-full rounded border border-slate-200 px-2 py-1 text-xs focus:border-indigo-500 focus:outline-none"
                placeholder="Describe how this agent should manage the session"
              />
              <p className="mt-2 text-[10px] font-semibold uppercase tracking-wider text-slate-500">
                Update cadence
              </p>
              <div className="mt-1 space-y-1">
                <label className="flex items-center gap-2">
                  <input
                    type="radio"
                    name={`edge-mode-${id}`}
                    value="idle-summary"
                    checked={updateMode === 'idle-summary'}
                    onChange={() => setUpdateMode('idle-summary')}
                  />
                  <span>After each idle period</span>
                </label>
                <label className="flex items-center gap-2">
                  <input
                    type="radio"
                    name={`edge-mode-${id}`}
                    value="push"
                    checked={updateMode === 'push'}
                    onChange={() => setUpdateMode('push')}
                  />
                  <span>Managed session pushes updates via MCP</span>
                </label>
                <label className="flex items-center gap-2">
                  <input
                    type="radio"
                    name={`edge-mode-${id}`}
                    value="poll"
                    checked={updateMode === 'poll'}
                    onChange={() => setUpdateMode('poll')}
                  />
                  <span>Poll every</span>
                  <input
                    type="number"
                    min={5}
                    value={pollFrequency}
                    onChange={(event) => setPollFrequency(Number(event.target.value) || 0)}
                    className="w-16 rounded border border-slate-200 px-1 py-0.5 text-right"
                  />
                  <span>seconds</span>
                </label>
              </div>
              <div className="mt-3 flex gap-2">
                <button
                  type="submit"
                  className="flex-1 rounded bg-indigo-600 px-2 py-1 text-[11px] font-semibold text-white hover:bg-indigo-500"
                  disabled={!instructions.trim()}
                >
                  Save
                </button>
                <button
                  type="button"
                  onClick={() => data.onDelete({ id })}
                  className="rounded border border-red-200 px-2 py-1 text-[11px] font-semibold text-red-600 hover:bg-red-50"
                >
                  Remove
                </button>
              </div>
            </form>
          ) : (
            <div
              onPointerDown={(event) => event.stopPropagation()}
              className="group flex items-center gap-2 rounded-full border border-slate-300 bg-white/95 px-3 py-1 text-[11px] font-medium text-slate-700 shadow"
            >
              <span className="text-[10px] uppercase tracking-wide text-indigo-600">
                {updateModeLabels[updateMode]}
              </span>
              {instructions ? <span className="text-slate-500">• {instructions.slice(0, 40)}{instructions.length > 40 ? '…' : ''}</span> : null}
              <button
                type="button"
                onClick={() => data.onEdit({ id })}
                className="rounded border border-transparent px-1 text-[10px] font-semibold text-indigo-600 hover:border-indigo-200 hover:bg-indigo-50"
              >
                Edit
              </button>
              <button
                type="button"
                onClick={() => data.onDelete({ id })}
                className="rounded border border-transparent px-1 text-[10px] font-semibold text-red-500 hover:border-red-200 hover:bg-red-50"
              >
                Remove
              </button>
            </div>
          )}
        </div>
      </EdgeLabelRenderer>
    </>
  );
});

export default RelationshipEdge;
