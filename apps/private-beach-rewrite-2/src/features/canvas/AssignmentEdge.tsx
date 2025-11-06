'use client';

import { memo, useMemo, useState } from 'react';
import {
  BaseEdge,
  EdgeLabelRenderer,
  getBezierPath,
  type EdgeProps,
} from 'reactflow';

export type UpdateMode = 'idle-summary' | 'push' | 'poll';

export type AssignmentEdgeData = {
  instructions: string;
  updateMode: UpdateMode;
  pollFrequency: number;
  isEditing: boolean;
  onSave: (payload: { id: string; instructions: string; updateMode: UpdateMode; pollFrequency: number }) => void;
  onEdit: (payload: { id: string }) => void;
  onDelete: (payload: { id: string }) => void;
};

const MODE_LABEL: Record<UpdateMode, string> = {
  'idle-summary': 'Idle summary',
  push: 'Managed session pushes',
  poll: 'Polling',
};

export const AssignmentEdge = memo(function AssignmentEdge({
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

  const handleSubmit = (event: React.FormEvent) => {
    event.preventDefault();
    data.onSave({ id, instructions: instructions.trim(), updateMode, pollFrequency });
  };

  const handleDelete = () => data.onDelete({ id });

  return (
    <>
      <BaseEdge path={edgePath} markerEnd={markerEnd} className="stroke-slate-400/40" />
      <EdgeLabelRenderer>
        <div
          className="pointer-events-auto"
          style={{ transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)` }}
        >
          {data.isEditing ? (
            <form
              onSubmit={handleSubmit}
              onPointerDown={(event) => event.stopPropagation()}
              className="w-72 rounded-2xl border border-slate-200 bg-white p-3 text-xs text-slate-700 shadow-2xl"
            >
              <p className="text-[10px] font-semibold uppercase tracking-widest text-slate-500">
                Assignment Instructions
              </p>
              <textarea
                value={instructions}
                onChange={(event) => setInstructions(event.target.value)}
                rows={3}
                className="mt-2 w-full rounded border border-slate-200 px-2 py-1 text-xs focus:border-indigo-500 focus:outline-none"
                placeholder="Describe how this agent should manage the connected session"
              />
              <p className="mt-3 text-[10px] font-semibold uppercase tracking-widest text-slate-500">
                Update cadence
              </p>
              <div className="mt-2 space-y-2">
                <label className="flex items-center gap-2">
                  <input
                    type="radio"
                    name={`edge-mode-${id}`}
                    value="idle-summary"
                    checked={updateMode === 'idle-summary'}
                    onChange={() => setUpdateMode('idle-summary')}
                  />
                  <span>Summarize whenever the session turns idle</span>
                </label>
                <label className="flex items-center gap-2">
                  <input
                    type="radio"
                    name={`edge-mode-${id}`}
                    value="push"
                    checked={updateMode === 'push'}
                    onChange={() => setUpdateMode('push')}
                  />
                  <span>Let the managed session push MCP updates</span>
                </label>
                <label className="flex flex-wrap items-center gap-2">
                  <span className="flex items-center gap-2">
                    <input
                      type="radio"
                      name={`edge-mode-${id}`}
                      value="poll"
                      checked={updateMode === 'poll'}
                      onChange={() => setUpdateMode('poll')}
                    />
                    <span>Poll every</span>
                  </span>
                  <input
                    type="number"
                    min={5}
                    value={pollFrequency}
                    onChange={(event) => setPollFrequency(Number(event.target.value) || 0)}
                    className="h-7 w-16 rounded border border-slate-200 px-1 text-right"
                  />
                  <span>seconds</span>
                </label>
              </div>
              <div className="mt-3 flex gap-2">
                <button
                  type="submit"
                  className="flex-1 rounded bg-indigo-600 px-3 py-1.5 text-[11px] font-semibold text-white hover:bg-indigo-500"
                  disabled={!instructions.trim()}
                >
                  Save
                </button>
                <button
                  type="button"
                  onClick={handleDelete}
                  className="rounded border border-red-300 px-3 py-1.5 text-[11px] font-semibold text-red-600 hover:bg-red-50"
                >
                  Remove
                </button>
              </div>
            </form>
          ) : (
            <div
              onPointerDown={(event) => event.stopPropagation()}
              className="group flex items-center gap-2 rounded-full border border-slate-200 bg-white/95 px-3 py-1 text-[11px] font-semibold text-slate-700 shadow"
            >
              <span className="text-[10px] uppercase tracking-wide text-indigo-600">
                {MODE_LABEL[updateMode]}
              </span>
              {instructions ? (
                <span className="text-slate-500">
                  • {instructions.length > 60 ? `${instructions.slice(0, 60)}…` : instructions}
                </span>
              ) : null}
              <button
                type="button"
                onClick={() => data.onEdit({ id })}
                className="rounded border border-transparent px-1 text-[10px] font-semibold text-indigo-600 hover:border-indigo-200 hover:bg-indigo-50"
              >
                Edit
              </button>
              <button
                type="button"
                onClick={handleDelete}
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
