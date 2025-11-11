'use client';

import { memo, useEffect, useMemo, useState } from 'react';
import {
  BaseEdge,
  EdgeLabelRenderer,
  getSmoothStepPath,
  type EdgeProps,
} from 'reactflow';
import { cn } from '@/lib/cn';
import { TILE_DANGER_BUTTON_CLASS, TILE_PRIMARY_BUTTON_CLASS } from '@/components/tileButtonClasses';

export type UpdateMode = 'idle-summary' | 'push' | 'poll';

export type AssignmentEdgeData = {
  instructions: string;
  updateMode: UpdateMode;
  pollFrequency: number;
  isEditing: boolean;
  status?: 'ok' | 'error';
  statusMessage?: string | null;
  onRetry?: (payload: { id: string }) => void;
  onShowTrace?: (payload: { id: string }) => void;
  onSave: (payload: { id: string; instructions: string; updateMode: UpdateMode; pollFrequency: number }) => void;
  onEdit: (payload: { id: string }) => void;
  onDelete: (payload: { id: string }) => void;
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
    () =>
      getSmoothStepPath({
        sourceX,
        sourceY,
        targetX,
        targetY,
        sourcePosition,
        targetPosition,
        borderRadius: 24,
      }),
    [sourcePosition, sourceX, sourceY, targetPosition, targetX, targetY],
  );
  const [instructions, setInstructions] = useState(data.instructions);
  const [updateMode, setUpdateMode] = useState<UpdateMode>(data.updateMode);
  const [pollFrequency, setPollFrequency] = useState<number>(data.pollFrequency);

  useEffect(() => {
    setInstructions(data.instructions);
  }, [data.instructions]);

  useEffect(() => {
    setUpdateMode(data.updateMode);
  }, [data.updateMode]);

  useEffect(() => {
    setPollFrequency(data.pollFrequency);
  }, [data.pollFrequency]);

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
          style={{
            transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)` ,
            zIndex: 1000,
            position: 'absolute',
          }}
        >
          {data.isEditing ? (
            <form
              onSubmit={handleSubmit}
              onPointerDown={(event) => event.stopPropagation()}
              className="w-72 rounded-2xl border border-slate-200/80 bg-white/95 p-4 text-[13px] text-slate-700 shadow-2xl dark:border-white/10 dark:bg-slate-950/95 dark:text-slate-200"
            >
              <p className="text-[11px] font-semibold uppercase tracking-[0.18em] text-slate-500 dark:text-slate-400">
                Assignment Instructions
              </p>
              <textarea
                value={instructions}
                onChange={(event) => setInstructions(event.target.value)}
                rows={3}
                className="mt-2 w-full rounded border border-slate-300 bg-white px-3 py-2 text-[13px] font-medium text-slate-900 placeholder:text-slate-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60 dark:border-white/10 dark:bg-white/5 dark:text-white dark:placeholder:text-slate-400"
                placeholder="Describe how this agent should manage the connected session"
              />
              <p className="mt-4 text-[11px] font-semibold uppercase tracking-[0.18em] text-slate-500 dark:text-slate-400">
                Update cadence
              </p>
              <div className="mt-2 space-y-3">
                <label className="flex items-center gap-2 text-[13px] text-slate-700 dark:text-slate-200">
                  <input
                    type="radio"
                    name={`edge-mode-${id}`}
                    value="idle-summary"
                    checked={updateMode === 'idle-summary'}
                    onChange={() => setUpdateMode('idle-summary')}
                    className="h-4 w-4 rounded-full border border-slate-300 text-indigo-500 focus:ring-indigo-400 dark:border-white/20 dark:bg-slate-900 dark:text-indigo-300"
                  />
                  <span>Summarize whenever the session turns idle</span>
                </label>
                <label className="flex items-center gap-2 text-[13px] text-slate-700 dark:text-slate-200">
                  <input
                    type="radio"
                    name={`edge-mode-${id}`}
                    value="push"
                    checked={updateMode === 'push'}
                    onChange={() => setUpdateMode('push')}
                    className="h-4 w-4 rounded-full border border-slate-300 text-indigo-500 focus:ring-indigo-400 dark:border-white/20 dark:bg-slate-900 dark:text-indigo-300"
                  />
                  <span>Let the managed session push MCP updates</span>
                </label>
                <label className="flex flex-wrap items-center gap-2 text-[13px] text-slate-700 dark:text-slate-200">
                  <span className="flex items-center gap-2">
                    <input
                      type="radio"
                      name={`edge-mode-${id}`}
                      value="poll"
                      checked={updateMode === 'poll'}
                      onChange={() => setUpdateMode('poll')}
                      className="h-4 w-4 rounded-full border border-slate-300 text-indigo-500 focus:ring-indigo-400 dark:border-white/20 dark:bg-slate-900 dark:text-indigo-300"
                    />
                    <span>Poll every</span>
                  </span>
                  <input
                    type="number"
                    min={1}
                    value={pollFrequency}
                    onChange={(event) => setPollFrequency(Number(event.target.value) || 0)}
                    className="h-9 w-20 rounded border border-slate-300 bg-white px-3 text-right text-[13px] font-medium text-slate-900 placeholder:text-slate-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60 dark:border-white/10 dark:bg-white/5 dark:text-white dark:placeholder:text-slate-400"
                  />
                  <span>seconds</span>
                </label>
              </div>
              <div className="mt-4 flex gap-2">
                <button
                  type="submit"
                  className={cn('flex-1', TILE_PRIMARY_BUTTON_CLASS)}
                  disabled={!instructions.trim()}
                >
                  Save
                </button>
                <button
                  type="button"
                  onClick={handleDelete}
                  className={TILE_DANGER_BUTTON_CLASS}
                >
                  Remove
                </button>
              </div>
            </form>
          ) : (
            <div className="flex flex-col items-center gap-2">
              <button
                type="button"
                onClick={() => data.onEdit({ id })}
                onPointerDown={(event) => event.stopPropagation()}
                className="inline-flex h-7 w-7 items-center justify-center rounded-full border border-white/60 bg-slate-900/80 text-sm text-white shadow-md transition hover:border-white/90 hover:bg-slate-900"
                aria-label="View assignment details"
              >
                ⓘ
              </button>
              {data.onShowTrace ? (
                <button
                  type="button"
                  onClick={(event) => {
                    event.stopPropagation();
                    data.onShowTrace?.({ id });
                  }}
                  className="inline-flex items-center justify-center rounded-full border border-sky-400/70 px-2 py-1 text-[10px] font-semibold uppercase tracking-[0.2em] text-sky-100 hover:border-sky-200"
                >
                  Trace
                </button>
              ) : null}
              {data.status === 'error' && data.statusMessage ? (
                <div className="w-48 rounded-xl border border-red-400/40 bg-red-500/10 px-3 py-2 text-[10px] text-red-100 shadow-lg">
                  <p className="font-semibold uppercase tracking-[0.2em]">Pairing failed</p>
                  <p className="mt-1">{data.statusMessage}</p>
                  {data.onRetry ? (
                    <button
                      type="button"
                      className="mt-2 inline-flex items-center justify-center rounded border border-red-200/60 px-2 py-1 text-[10px] font-semibold uppercase tracking-[0.2em] text-red-100 hover:border-red-100 hover:text-white"
                      onClick={(event) => {
                        event.stopPropagation();
                        data.onRetry?.({ id });
                      }}
                    >
                      Retry
                    </button>
                  ) : null}
                </div>
              ) : null}
            </div>
          )}
        </div>
      </EdgeLabelRenderer>
    </>
  );
});
