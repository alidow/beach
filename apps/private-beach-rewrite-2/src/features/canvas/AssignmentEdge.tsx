'use client';

import { memo, useEffect, useMemo, useState } from 'react';
import {
  BaseEdge,
  EdgeLabelRenderer,
  getSmoothStepPath,
  type EdgeProps,
} from 'reactflow';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Textarea } from '@/components/ui/textarea';
import type { RelationshipCadenceConfig, RelationshipUpdateMode } from '../tiles/types';
import { createDefaultRelationshipCadence, inferUpdateModeFromCadence } from '../tiles/types';
import { cadenceHasAnyPath, formatCadenceSummary } from './updateCadence';

export type AssignmentEdgeConnectionState = 'pending' | 'slow' | 'fast' | 'error';

type EdgeStrokeStyle = {
  stroke: string;
  strokeWidth: number;
};

const DEFAULT_EDGE_STYLE: EdgeStrokeStyle = {
  stroke: 'rgba(148, 163, 184, 0.5)',
  strokeWidth: 2,
};

const CONNECTION_STYLES: Record<AssignmentEdgeConnectionState, EdgeStrokeStyle> = {
  error: { stroke: '#ff1744', strokeWidth: 4 },
  pending: { stroke: '#ffd60a', strokeWidth: 4 },
  slow: { stroke: '#39ff14', strokeWidth: 3.2 },
  fast: { stroke: '#ff5de5', strokeWidth: 4 },
};

function resolveEdgeStyle(state?: AssignmentEdgeConnectionState | null): EdgeStrokeStyle {
  if (!state) {
    return DEFAULT_EDGE_STYLE;
  }
  return CONNECTION_STYLES[state] ?? DEFAULT_EDGE_STYLE;
}

export type AssignmentEdgeData = {
  instructions: string;
  updateMode: RelationshipUpdateMode;
  pollFrequency: number;
  cadence: RelationshipCadenceConfig;
  isEditing: boolean;
  status?: 'ok' | 'error';
  statusMessage?: string | null;
  onRetry?: (payload: { id: string }) => void;
  onShowTrace?: (payload: { id: string }) => void;
  connectionState?: AssignmentEdgeConnectionState;
  connectionMessage?: string | null;
  onSave: (payload: {
    id: string;
    instructions: string;
    updateMode: RelationshipUpdateMode;
    pollFrequency: number;
    cadence: RelationshipCadenceConfig;
  }) => void;
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
  const fallbackEdgeData = useMemo<AssignmentEdgeData>(
    () => ({
      instructions: '',
      updateMode: 'poll',
      pollFrequency: 30,
      cadence: createDefaultRelationshipCadence(),
      isEditing: false,
      status: 'ok',
      statusMessage: null,
      connectionState: 'pending',
      onSave: () => {},
      onEdit: () => {},
      onDelete: () => {},
    }),
    [],
  );
  const edgeData = data ?? fallbackEdgeData;
  const [instructions, setInstructions] = useState(edgeData.instructions);
  const [cadence, setCadence] = useState<RelationshipCadenceConfig>(() => ({
    ...(edgeData.cadence ?? createDefaultRelationshipCadence()),
  }));
  const edgeStyle = resolveEdgeStyle(edgeData.connectionState);
  const hasConnectionError = edgeData.connectionState === 'error';
  const failureMessage = edgeData.connectionMessage ?? edgeData.statusMessage ?? 'Unknown error.';
  const [showErrorPopover, setShowErrorPopover] = useState(false);
  const [showErrorDetails, setShowErrorDetails] = useState(false);
  const marker = markerEnd ? { ...markerEnd, color: edgeStyle.stroke } : markerEnd;

  useEffect(() => {
    setInstructions(edgeData.instructions);
  }, [edgeData.instructions]);

  useEffect(() => {
    setCadence({ ...(edgeData.cadence ?? createDefaultRelationshipCadence()) });
  }, [edgeData.cadence]);

  useEffect(() => {
    if (!hasConnectionError) {
      setShowErrorPopover(false);
      setShowErrorDetails(false);
    }
  }, [hasConnectionError]);

  const handleSubmit = (event: React.FormEvent) => {
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

  const handleDelete = () => edgeData.onDelete({ id });
  const hasCadence = cadenceHasAnyPath(cadence);

  return (
    <>
      <BaseEdge
        path={edgePath}
        markerEnd={marker}
        style={{
          stroke: edgeStyle.stroke,
          strokeWidth: edgeStyle.strokeWidth,
          strokeLinecap: 'round',
        }}
      />
      <EdgeLabelRenderer>
        <div
          className="pointer-events-auto"
          style={{
            transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)` ,
            zIndex: 1000,
            position: 'absolute',
          }}
        >
          {hasConnectionError ? (
            <div className="mb-2 flex flex-col items-center">
              <button
                type="button"
                onClick={(event) => {
                  event.stopPropagation();
                  setShowErrorPopover((prev) => !prev);
                }}
                className="inline-flex h-8 w-8 items-center justify-center rounded-full border border-rose-400/80 bg-rose-600 text-lg font-semibold text-white shadow-lg transition hover:scale-105"
                aria-label="Show connection failure details"
              >
                !
              </button>
              {showErrorPopover ? (
                <div className="mt-3 w-60 rounded-2xl border border-rose-400/60 bg-rose-600/90 p-3 text-rose-50 shadow-2xl">
                  <p className="text-sm font-semibold uppercase tracking-[0.25em] text-rose-100">Connection failed</p>
                  <p className="mt-1 text-[12px] text-rose-50/90">
                    The controller was unable to connect to this child session.
                  </p>
                  <button
                    type="button"
                    className="mt-2 inline-flex items-center gap-1 text-[11px] font-semibold uppercase tracking-[0.25em] text-rose-100 underline"
                    onClick={(event) => {
                      event.stopPropagation();
                      setShowErrorDetails((prev) => !prev);
                    }}
                  >
                    {showErrorDetails ? 'Hide details' : 'More details'}
                  </button>
                  {showErrorDetails ? (
                    <div className="mt-2 rounded-lg border border-rose-200/40 bg-rose-500/30 p-2 text-[11px] text-rose-50">
                      {failureMessage}
                    </div>
                  ) : null}
                </div>
              ) : null}
            </div>
          ) : null}
          {edgeData.isEditing ? (
            <form
              onSubmit={handleSubmit}
              onPointerDown={(event) => event.stopPropagation()}
              className="w-72 space-y-4 rounded-2xl border border-border/70 bg-card/95 p-4 text-[13px] text-card-foreground shadow-2xl dark:border-white/10 dark:bg-slate-950/95"
            >
              <div className="space-y-2">
                <Label htmlFor={`${id}-instructions`} className="text-[11px] text-muted-foreground">
                  Manager Instructions
                </Label>
                <Textarea
                  id={`${id}-instructions`}
                  value={instructions}
                  onChange={(event) => setInstructions(event.target.value)}
                  rows={3}
                  placeholder="Describe how this agent should manage the connected session (optional)"
                  className="min-h-[96px] text-[13px] font-medium"
                />
              </div>
              <div className="space-y-3">
                <Label className="text-[11px] text-muted-foreground">Update cadence</Label>
                <label className="flex items-start gap-3 text-[13px] text-foreground/90 dark:text-slate-200">
                  <input
                    type="checkbox"
                    className="mt-0.5 h-4 w-4 rounded border border-border text-indigo-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-400"
                    checked={cadence.idleSummary}
                    onChange={(event) =>
                      setCadence((prev) => ({ ...prev, idleSummary: event.target.checked }))
                    }
                  />
                  <span>Update whenever the child tile becomes idle</span>
                </label>
                <label className="flex items-start gap-3 text-[13px] text-foreground/90 dark:text-slate-200">
                  <input
                    type="checkbox"
                    className="mt-0.5 h-4 w-4 rounded border border-border text-indigo-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-400"
                    checked={cadence.allowChildPush}
                    onChange={(event) =>
                      setCadence((prev) => ({ ...prev, allowChildPush: event.target.checked }))
                    }
                  />
                  <span>Allow the managed session to push MCP updates</span>
                </label>
                <div className="rounded-xl border border-border/80 bg-background/40 px-3 py-2">
                  <label className="flex flex-wrap items-center gap-3 text-[13px] text-foreground/90 dark:text-slate-200">
                    <input
                      type="checkbox"
                      className="mt-0.5 h-4 w-4 rounded border border-border text-indigo-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-400"
                      checked={cadence.pollEnabled}
                      onChange={(event) =>
                        setCadence((prev) => ({ ...prev, pollEnabled: event.target.checked }))
                      }
                    />
                    <span className="flex-1 min-w-[120px]">Poll every</span>
                    <Input
                      type="number"
                      min={1}
                      value={cadence.pollFrequencySeconds}
                      onChange={(event) => {
                        const next = Math.max(1, Math.round(Number(event.target.value) || 0));
                        setCadence((prev) => ({ ...prev, pollFrequencySeconds: next }));
                      }}
                      disabled={!cadence.pollEnabled}
                      className="h-8 w-20 text-right text-[13px] font-medium disabled:opacity-50"
                    />
                    <span className="text-[12px] text-muted-foreground">seconds</span>
                  </label>
                  <p className="ml-7 mt-2 text-[11px] text-muted-foreground">
                    Only poll if the child changed and there were no idle or MCP updates during the
                    last 30 seconds.
                  </p>
                </div>
              </div>
              <div className="mt-2 flex gap-2">
                <Button
                  type="submit"
                  className="flex-1 text-[11px] font-semibold uppercase tracking-[0.2em]"
                  disabled={!hasCadence}
                >
                  Save
                </Button>
                <Button
                  type="button"
                  variant="destructive"
                  onClick={handleDelete}
                  className="flex-1 text-[11px] font-semibold uppercase tracking-[0.18em]"
                >
                  Remove
                </Button>
              </div>
            </form>
          ) : (
            <div className="flex flex-col items-center gap-2">
              <button
                type="button"
                onClick={() => edgeData.onEdit({ id })}
                onPointerDown={(event) => event.stopPropagation()}
                className="inline-flex h-7 w-7 items-center justify-center rounded-full border border-white/60 bg-slate-900/80 text-sm text-white shadow-md transition hover:border-white/90 hover:bg-slate-900"
                aria-label="View assignment details"
              >
                ⓘ
              </button>
              <p className="max-w-[10rem] text-center text-[11px] font-medium text-white/80">
                {formatCadenceSummary(edgeData.cadence)}
              </p>
              {edgeData.onShowTrace ? (
                <button
                  type="button"
                  onClick={(event) => {
                    event.stopPropagation();
                    edgeData.onShowTrace?.({ id });
                  }}
                  className="inline-flex items-center justify-center rounded-full border border-sky-400/70 px-2 py-1 text-[10px] font-semibold uppercase tracking-[0.2em] text-sky-100 hover:border-sky-200"
                >
                  Trace
                </button>
              ) : null}
              {edgeData.status === 'error' && edgeData.statusMessage ? (
                <div className="w-48 rounded-xl border border-red-400/40 bg-red-500/10 px-3 py-2 text-[10px] text-red-100 shadow-lg">
                  <p className="font-semibold uppercase tracking-[0.2em]">Pairing failed</p>
                  <p className="mt-1">{edgeData.statusMessage}</p>
                  {edgeData.onRetry ? (
                    <button
                      type="button"
                      className="mt-2 inline-flex items-center justify-center rounded border border-red-200/60 px-2 py-1 text-[10px] font-semibold uppercase tracking-[0.2em] text-red-100 hover:border-red-100 hover:text-white"
                      onClick={(event) => {
                        event.stopPropagation();
                        edgeData.onRetry?.({ id });
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
