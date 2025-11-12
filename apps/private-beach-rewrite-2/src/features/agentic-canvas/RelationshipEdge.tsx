'use client';

import { memo, useEffect, useMemo, useState } from 'react';
import {
  BaseEdge,
  EdgeLabelRenderer,
  getBezierPath,
  type EdgeProps,
} from 'reactflow';
import type { AssignmentEdgeData } from './types';
import { createDefaultRelationshipCadence, inferUpdateModeFromCadence } from '../tiles/types';
import { cadenceHasAnyPath, formatCadenceSummary } from '../canvas/updateCadence';

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
  const fallbackEdgeData = useMemo<AssignmentEdgeData>(
    () => ({
      instructions: '',
      updateMode: 'poll',
      pollFrequency: 30,
      cadence: createDefaultRelationshipCadence(),
      isEditing: false,
      onSave: () => {},
      onEdit: () => {},
      onDelete: () => {},
    }),
    [],
  );
  const edgeData = data ?? fallbackEdgeData;
  const [instructions, setInstructions] = useState(edgeData.instructions);
  const [cadence, setCadence] = useState(() => ({ ...(edgeData.cadence ?? createDefaultRelationshipCadence()) }));

  useEffect(() => {
    setInstructions(edgeData.instructions);
    setCadence({ ...(edgeData.cadence ?? createDefaultRelationshipCadence()) });
  }, [edgeData.cadence, edgeData.instructions]);

  const isEditing = edgeData.isEditing;

  const onSubmit = (event: React.FormEvent) => {
    event.preventDefault();
    const trimmed = instructions.trim();
    const nextCadence = { ...cadence };
    const nextMode = inferUpdateModeFromCadence(nextCadence);
    edgeData.onSave({
      id,
      instructions: trimmed,
      updateMode: nextMode,
      pollFrequency: nextCadence.pollFrequencySeconds,
      cadence: nextCadence,
    });
  };
  const hasCadence = cadenceHasAnyPath(cadence);

  return (
    <>
      <BaseEdge path={edgePath} markerEnd={markerEnd} style={{ stroke: '#cbd5f5' }} />
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
                Manager Instructions
              </p>
              <textarea
                value={instructions}
                onChange={(event) => setInstructions(event.target.value)}
                rows={3}
                className="mt-2 w-full rounded border border-slate-200 bg-white px-2 py-1 text-xs text-slate-900 placeholder:text-slate-400 focus:border-indigo-500 focus:outline-none dark:bg-slate-900 dark:text-slate-100 dark:placeholder:text-slate-500"
                placeholder="Describe how this agent should manage the session (optional)"
              />
              <p className="mt-2 text-[10px] font-semibold uppercase tracking-wider text-slate-500">
                Update cadence
              </p>
              <div className="mt-1 space-y-1">
                <label className="flex items-center gap-2">
                  <input
                    type="checkbox"
                    checked={cadence.idleSummary}
                    onChange={(event) =>
                      setCadence((prev) => ({ ...prev, idleSummary: event.target.checked }))
                    }
                  />
                  <span>After each idle period</span>
                </label>
                <label className="flex items-center gap-2">
                  <input
                    type="checkbox"
                    checked={cadence.allowChildPush}
                    onChange={(event) =>
                      setCadence((prev) => ({ ...prev, allowChildPush: event.target.checked }))
                    }
                  />
                  <span>Managed session pushes updates via MCP</span>
                </label>
                <label className="flex flex-wrap items-center gap-2">
                  <input
                    type="checkbox"
                    checked={cadence.pollEnabled}
                    onChange={(event) =>
                      setCadence((prev) => ({ ...prev, pollEnabled: event.target.checked }))
                    }
                  />
                  <span>Poll every</span>
                  <input
                    type="number"
                    min={1}
                    value={cadence.pollFrequencySeconds}
                    onChange={(event) => {
                      const next = Math.max(1, Math.round(Number(event.target.value) || 0));
                      setCadence((prev) => ({ ...prev, pollFrequencySeconds: next }));
                    }}
                    className="w-16 rounded border border-slate-200 bg-white px-1 py-0.5 text-right text-slate-900 placeholder:text-slate-400 focus:border-indigo-500 focus:outline-none disabled:opacity-50 dark:bg-slate-900 dark:text-slate-100 dark:placeholder:text-slate-500"
                    disabled={!cadence.pollEnabled}
                  />
                  <span>seconds</span>
                </label>
                <p className="ml-5 text-[10px] text-slate-500">
                  Polls only happen if the child changed and no idle or MCP update was sent in the
                  last 30s.
                </p>
              </div>
              <div className="mt-3 flex gap-2">
                <button
                  type="submit"
                  className="flex-1 rounded bg-indigo-600 px-2 py-1 text-[11px] font-semibold text-white hover:bg-indigo-500"
                  disabled={!hasCadence}
                >
                  Save
                </button>
                <button
                  type="button"
                  onClick={() => edgeData.onDelete({ id })}
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
                {formatCadenceSummary(edgeData.cadence)}
              </span>
              {instructions ? <span className="text-slate-500">• {instructions.slice(0, 40)}{instructions.length > 40 ? '…' : ''}</span> : null}
              <button
                type="button"
                onClick={() => edgeData.onEdit({ id })}
                className="rounded border border-transparent px-1 text-[10px] font-semibold text-indigo-600 hover:border-indigo-200 hover:bg-indigo-50"
              >
                Edit
              </button>
              <button
                type="button"
                onClick={() => edgeData.onDelete({ id })}
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
