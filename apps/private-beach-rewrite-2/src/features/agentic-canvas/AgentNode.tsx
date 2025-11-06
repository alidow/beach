'use client';

import { memo, useEffect, useState } from 'react';
import { Handle, Position } from 'reactflow';
import type { AgentNodeData } from './types';

const AgentNode = memo(function AgentNode({ data }: { data: AgentNodeData }) {
  const [role, setRole] = useState(data.role);
  const [responsibility, setResponsibility] = useState(data.responsibility);

  useEffect(() => {
    if (!data.isEditing) {
      setRole(data.role);
      setResponsibility(data.responsibility);
    }
  }, [data.isEditing, data.responsibility, data.role]);

  const showEditor = data.isEditing;

  return (
    <div className="flex h-full w-full flex-col rounded-xl border border-slate-200 bg-white shadow-lg">
      <div className="flex items-center justify-between border-b border-slate-100 px-3 py-2 text-xs font-semibold uppercase tracking-wide text-slate-500">
        <span>{data.label}</span>
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
              data.onSave({ id: data.id, role: role.trim(), responsibility: responsibility.trim() });
            }}
          >
            <label className="text-xs font-semibold text-slate-500" htmlFor={`role-${data.id}`}>
              Role
            </label>
            <input
              id={`role-${data.id}`}
              name="role"
              value={role}
              onChange={(event) => setRole(event.target.value)}
              className="rounded border border-slate-200 px-2 py-1 text-sm focus:border-indigo-500 focus:outline-none"
              placeholder="e.g. On-call orchestrator"
            />
            <label className="text-xs font-semibold text-slate-500" htmlFor={`resp-${data.id}`}>
              Responsibility
            </label>
            <textarea
              id={`resp-${data.id}`}
              name="responsibility"
              value={responsibility}
              onChange={(event) => setResponsibility(event.target.value)}
              rows={3}
              className="min-h-[80px] rounded border border-slate-200 px-2 py-1 text-sm focus:border-indigo-500 focus:outline-none"
              placeholder="Describe how this agent should behave"
            />
            <div className="mt-auto flex gap-2 pt-1 text-xs">
              <button
                type="submit"
                className="flex-1 rounded bg-indigo-600 px-2 py-1 font-semibold text-white hover:bg-indigo-500"
                disabled={!role.trim() || !responsibility.trim()}
              >
                Save
              </button>
              <button
                type="button"
                onClick={() => data.onCancel({ id: data.id })}
                className="flex-1 rounded border border-slate-200 px-2 py-1 font-semibold text-slate-600 hover:bg-slate-50"
              >
                Cancel
              </button>
            </div>
          </form>
        ) : (
          <>
            <div>
              <p className="text-xs font-semibold uppercase text-slate-500">Role</p>
              <p className="text-sm text-slate-800">{data.role || '—'}</p>
            </div>
            <div>
              <p className="text-xs font-semibold uppercase text-slate-500">Responsibility</p>
              <p className="text-sm text-slate-800">{data.responsibility || '—'}</p>
            </div>
          </>
        )}
      </div>
      <Handle type="target" position={Position.Left} className="h-3 w-3 border-none bg-indigo-400" />
      <Handle type="source" position={Position.Right} className="h-3 w-3 border-none bg-indigo-600" />
    </div>
  );
});

export default AgentNode;
