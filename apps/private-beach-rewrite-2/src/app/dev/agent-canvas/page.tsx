import type { Metadata } from 'next';
import { AgentCanvas } from '@/features/agentic-canvas';

export const metadata: Metadata = {
  title: 'Agent Canvas Sandbox',
};

export default function AgentCanvasSandboxPage() {
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-6 px-6 py-8">
      <div className="space-y-2">
        <p className="text-xs font-semibold uppercase tracking-widest text-slate-500">Experimental</p>
        <h1 className="text-2xl font-semibold text-slate-900">Agent Canvas Sandbox</h1>
        <p className="text-sm text-slate-600">
          Use the buttons below to add agents or applications, then drag the connector handle on an agent to define
          relationships. This page is powered entirely by client-side mock data.
        </p>
      </div>
      <AgentCanvas />
    </div>
  );
}
