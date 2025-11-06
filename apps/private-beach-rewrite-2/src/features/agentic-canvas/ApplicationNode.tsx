'use client';

import { memo, useEffect, useState } from 'react';
import { Handle, Position } from 'reactflow';
import type { ApplicationNodeData } from './types';

const ApplicationNode = memo(function ApplicationNode({ data }: { data: ApplicationNodeData }) {
  const [label, setLabel] = useState(data.label);
  const [description, setDescription] = useState(data.description);

  useEffect(() => {
    if (!data.isEditing) {
      setLabel(data.label);
      setDescription(data.description);
    }
  }, [data.description, data.isEditing, data.label]);

  const showEditor = data.isEditing;

  return (
    <div className="flex h-full w-full flex-col rounded-xl border border-slate-200 bg-slate-50 shadow">
      <div className="flex items-center justify-between border-b border-slate-200 px-3 py-2 text-xs font-semibold uppercase tracking-wide text-slate-600">
        <span>Application</span>
        {!showEditor && (
          <button
            type="button"
            onClick={() => data.onEdit({ id: data.id })}
            className="rounded border border-transparent px-2 py-1 text-[10px] font-semibold text-indigo-600 hover:border-indigo-200 hover:bg-indigo-50"
          >
            Edit
          </button>
        )}
      </div>
      <div className="flex flex-1 flex-col gap-2 px-3 py-3 text-sm text-slate-700">
        {showEditor ? (
          <form
            className="flex flex-1 flex-col gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              data.onSave({ id: data.id, label: label.trim(), description: description.trim() });
            }}
          >
            <label className="text-xs font-semibold text-slate-500" htmlFor={`app-label-${data.id}`}>
              Name
            </label>
            <input
              id={`app-label-${data.id}`}
              value={label}
              onChange={(event) => setLabel(event.target.value)}
              className="rounded border border-slate-200 px-2 py-1 text-sm focus:border-indigo-500 focus:outline-none"
              placeholder="e.g. Deploy dashboard"
            />
            <label className="text-xs font-semibold text-slate-500" htmlFor={`app-desc-${data.id}`}>
              Description
            </label>
            <textarea
              id={`app-desc-${data.id}`}
              value={description}
              onChange={(event) => setDescription(event.target.value)}
              rows={3}
              className="min-h-[80px] rounded border border-slate-200 px-2 py-1 text-sm focus:border-indigo-500 focus:outline-none"
              placeholder="Tell agents what this session does"
            />
            <div className="mt-auto flex gap-2 pt-1 text-xs">
              <button
                type="submit"
                className="flex-1 rounded bg-indigo-600 px-2 py-1 font-semibold text-white hover:bg-indigo-500"
                disabled={!label.trim()}
              >
                Save
              </button>
              <button
                type="button"
                onClick={() => data.onCancel({ id: data.id })}
                className="flex-1 rounded border border-slate-200 px-2 py-1 font-semibold text-slate-600 hover:bg-slate-100"
              >
                Cancel
              </button>
            </div>
          </form>
        ) : (
          <>
            <div>
              <p className="text-xs font-semibold uppercase text-slate-500">Name</p>
              <p className="text-sm text-slate-800">{data.label || '—'}</p>
            </div>
            <div>
              <p className="text-xs font-semibold uppercase text-slate-500">Purpose</p>
              <p className="text-sm text-slate-800">{data.description || '—'}</p>
            </div>
          </>
        )}
      </div>
      <Handle type="target" position={Position.Left} className="h-3 w-3 border-none bg-emerald-500" />
    </div>
  );
});

export default ApplicationNode;
