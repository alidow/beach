'use client';

import 'reactflow/dist/style.css';

import { useCallback, useMemo, useRef, useState } from 'react';
import ReactFlow, {
  Background,
  Controls,
  ReactFlowProvider,
  addEdge,
  applyEdgeChanges,
  applyNodeChanges,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node,
  type NodeChange,
} from 'reactflow';
import AgentNode from './AgentNode';
import ApplicationNode from './ApplicationNode';
import RelationshipEdge from './RelationshipEdge';
import type { AgentNodeData, ApplicationNodeData, AssignmentEdgeData, UpdateMode } from './types';

const AGENT_NODE_SIZE = { width: 260, height: 240 };
const APPLICATION_NODE_SIZE = { width: 230, height: 200 };

// Stable maps to avoid React Flow #002 remounts during drag
const NODE_TYPES = Object.freeze({ agent: AgentNode, application: ApplicationNode });
const EDGE_TYPES = Object.freeze({ assignment: RelationshipEdge });

function AgentCanvasInner() {
  const [nodes, setNodes] = useState<Array<Node<AgentNodeData | ApplicationNodeData>>>([]);
  const [edges, setEdges] = useState<Array<Edge<AssignmentEdgeData>>>([]);
  const agentCounterRef = useRef(1);
  const applicationCounterRef = useRef(1);

  const buildPosition = useCallback(
    (index: number) => ({
      x: 40 + ((index % 4) * 280),
      y: 40 + (Math.floor(index / 4) * 260),
    }),
    [],
  );

  const updateNodeData = useCallback(
    (id: string, updater: (data: AgentNodeData | ApplicationNodeData) => AgentNodeData | ApplicationNodeData) => {
      setNodes((current) => current.map((node) => (node.id === id ? { ...node, data: updater(node.data) } : node)));
    },
    [],
  );

  const removeNodeIfDraft = useCallback((id: string) => {
    setNodes((current) => current.filter((node) => {
      if (node.id !== id) return true;
      const data = node.data;
      if ('role' in data) {
        return Boolean(data.role || data.responsibility);
      }
      if ('label' in data) {
        return Boolean(data.label);
      }
      return true;
    }));
  }, []);

  const handleAgentSave = useCallback(
    ({ id, role, responsibility }: { id: string; role: string; responsibility: string }) => {
      updateNodeData(id, (data) => ({
        ...(data as AgentNodeData),
        role,
        responsibility,
        isEditing: false,
      }));
    },
    [updateNodeData],
  );

  const handleAgentCancel = useCallback(
    ({ id }: { id: string }) => {
      updateNodeData(id, (data) => ({ ...(data as AgentNodeData), isEditing: false }));
      removeNodeIfDraft(id);
    },
    [removeNodeIfDraft, updateNodeData],
  );

  const handleAgentEdit = useCallback(
    ({ id }: { id: string }) => {
      updateNodeData(id, (data) => ({ ...(data as AgentNodeData), isEditing: true }));
    },
    [updateNodeData],
  );

  const handleApplicationSave = useCallback(
    ({ id, label, description }: { id: string; label: string; description: string }) => {
      updateNodeData(id, (data) => ({
        ...(data as ApplicationNodeData),
        label,
        description,
        isEditing: false,
      }));
    },
    [updateNodeData],
  );

  const handleApplicationCancel = useCallback(
    ({ id }: { id: string }) => {
      updateNodeData(id, (data) => ({ ...(data as ApplicationNodeData), isEditing: false }));
      removeNodeIfDraft(id);
    },
    [removeNodeIfDraft, updateNodeData],
  );

  const handleApplicationEdit = useCallback(
    ({ id }: { id: string }) => {
      updateNodeData(id, (data) => ({ ...(data as ApplicationNodeData), isEditing: true }));
    },
    [updateNodeData],
  );

  const handleEdgeSave = useCallback(
    ({ id, instructions, updateMode, pollFrequency }: { id: string; instructions: string; updateMode: UpdateMode; pollFrequency: number }) => {
      setEdges((current) =>
        current.map((edge) =>
          edge.id === id
            ? {
                ...edge,
                data: {
                  ...edge.data,
                  instructions,
                  updateMode,
                  pollFrequency,
                  isEditing: false,
                },
              }
            : edge,
        ),
      );
    },
    [],
  );

  const handleEdgeEdit = useCallback(({ id }: { id: string }) => {
    setEdges((current) =>
      current.map((edge) =>
        edge.id === id
          ? {
              ...edge,
              data: {
                ...edge.data,
                isEditing: true,
              },
            }
          : edge,
      ),
    );
  }, []);

  const handleEdgeDelete = useCallback(({ id }: { id: string }) => {
    setEdges((current) => current.filter((edge) => edge.id !== id));
  }, []);

  const createAgentNode = useCallback(() => {
    const id = `agent-${agentCounterRef.current++}`;
    const position = buildPosition(nodes.length);
    const label = `Agent ${agentCounterRef.current - 1}`;
    const node: Node<AgentNodeData> = {
      id,
      type: 'agent',
      position,
      data: {
        id,
        label,
        role: '',
        responsibility: '',
        isEditing: true,
        onSave: handleAgentSave,
        onCancel: handleAgentCancel,
        onEdit: handleAgentEdit,
      },
      draggable: true,
      style: { width: AGENT_NODE_SIZE.width, height: AGENT_NODE_SIZE.height },
    };
    setNodes((current) => [...current, node]);
  }, [buildPosition, handleAgentCancel, handleAgentEdit, handleAgentSave, nodes.length]);

  const createApplicationNode = useCallback(() => {
    const id = `app-${applicationCounterRef.current++}`;
    const position = buildPosition(nodes.length);
    const label = `Application ${applicationCounterRef.current - 1}`;
    const node: Node<ApplicationNodeData> = {
      id,
      type: 'application',
      position,
      data: {
        id,
        label,
        description: '',
        isEditing: true,
        onSave: handleApplicationSave,
        onCancel: handleApplicationCancel,
        onEdit: handleApplicationEdit,
      },
      draggable: true,
      style: { width: APPLICATION_NODE_SIZE.width, height: APPLICATION_NODE_SIZE.height },
    };
    setNodes((current) => [...current, node]);
  }, [buildPosition, handleApplicationCancel, handleApplicationEdit, handleApplicationSave, nodes.length]);

  const onNodesChange = useCallback(
    (changes: NodeChange[]) => setNodes((nds) => applyNodeChanges(changes, nds)),
    [],
  );

  const onEdgesChange = useCallback(
    (changes: EdgeChange[]) => setEdges((eds) => applyEdgeChanges(changes, eds)),
    [],
  );

  const onConnect = useCallback(
    (connection: Connection) => {
      if (!connection.source || !connection.target) return;
      const sourceNode = nodes.find((node) => node.id === connection.source);
      const targetNode = nodes.find((node) => node.id === connection.target);
      if (!sourceNode || !targetNode) return;
      if (sourceNode.type !== 'agent') {
        return;
      }
      const edgeId = `edge-${Date.now()}-${Math.round(Math.random() * 1000)}`;
      const edge: Edge<AssignmentEdgeData> = {
        id: edgeId,
        type: 'assignment',
        source: connection.source,
        target: connection.target,
        data: {
          instructions: '',
          updateMode: 'idle-summary',
          pollFrequency: 60,
          isEditing: true,
          onSave: handleEdgeSave,
          onEdit: handleEdgeEdit,
          onDelete: handleEdgeDelete,
        },
      };
      setEdges((current) => addEdge(edge, current));
    },
    [handleEdgeDelete, handleEdgeEdit, handleEdgeSave, nodes],
  );

  const memoNodeTypes = useMemo(() => NODE_TYPES, []);
  const memoEdgeTypes = useMemo(() => EDGE_TYPES, []);

  return (
    <div className="flex h-full flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={createAgentNode}
          className="rounded bg-indigo-600 px-3 py-1.5 text-sm font-semibold text-white hover:bg-indigo-500"
        >
          Add Agent
        </button>
        <button
          type="button"
          onClick={createApplicationNode}
          className="rounded border border-slate-300 px-3 py-1.5 text-sm font-semibold text-slate-700 hover:bg-slate-100"
        >
          Add Application
        </button>
        <span className="text-xs text-slate-500">Drag the handle on an agent to connect it to another node.</span>
      </div>
      <div className="h-[70vh] w-full rounded-2xl border border-slate-200 bg-white">
        <ReactFlow
          nodes={nodes}
          edges={edges}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          nodeTypes={memoNodeTypes}
          edgeTypes={memoEdgeTypes}
          fitView
          fitViewOptions={{ padding: 0.2 }}
          minZoom={0.4}
          maxZoom={1.5}
        >
          <Background gap={16} color="#dee2e6" />
          <Controls showInteractive={false} />
        </ReactFlow>
      </div>
    </div>
  );
}

export function AgentCanvas() {
  return (
    <ReactFlowProvider>
      <AgentCanvasInner />
    </ReactFlowProvider>
  );
}
