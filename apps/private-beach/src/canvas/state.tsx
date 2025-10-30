'use client';

import { createContext, useCallback, useContext, useMemo, useReducer } from 'react';
import type { CanvasEdge, CanvasLayoutV3, CanvasNodeBase, CanvasViewport } from './types';
import type { BatchAssignmentResponse } from './assignments';

export type CanvasState = {
  nodes: CanvasNodeBase[];
  edges: CanvasEdge[];
  viewport: CanvasViewport;
  selection: string[];
};

type Action =
  | { type: 'load'; payload: CanvasLayoutV3 }
  | { type: 'setNodes'; payload: CanvasNodeBase[] }
  | { type: 'updateNode'; payload: { id: string; patch: Partial<CanvasNodeBase> } }
  | { type: 'setEdges'; payload: CanvasEdge[] }
  | { type: 'setViewport'; payload: CanvasViewport }
  | { type: 'setSelection'; payload: string[] };

const defaultState: CanvasState = {
  nodes: [],
  edges: [],
  viewport: { x: 0, y: 0, zoom: 1 },
  selection: [],
};

function reducer(state: CanvasState, action: Action): CanvasState {
  switch (action.type) {
    case 'load': {
      const { nodes, edges, viewport } = action.payload;
      return {
        nodes: nodes ?? [],
        edges: edges ?? [],
        viewport: viewport ?? { x: 0, y: 0, zoom: 1 },
        selection: [],
      };
    }
    case 'setNodes':
      return { ...state, nodes: action.payload };
    case 'updateNode': {
      const { id, patch } = action.payload;
      return {
        ...state,
        nodes: state.nodes.map((n) => (n.id === id ? { ...n, ...patch } : n)),
      };
    }
    case 'setEdges':
      return { ...state, edges: action.payload };
    case 'setViewport':
      return { ...state, viewport: action.payload };
    case 'setSelection':
      return { ...state, selection: action.payload };
    default:
      return state;
  }
}

const StateCtx = createContext<CanvasState>(defaultState);
const DispatchCtx = createContext<React.Dispatch<Action>>(() => {});

export function CanvasProvider({ children }: { children: React.ReactNode }) {
  const [state, dispatch] = useReducer(reducer, defaultState);
  const stateValue = useMemo(() => state, [state]);
  return (
    <StateCtx.Provider value={stateValue}>
      <DispatchCtx.Provider value={dispatch}>{children}</DispatchCtx.Provider>
    </StateCtx.Provider>
  );
}

export function useCanvasState() {
  return useContext(StateCtx);
}

export function useCanvasActions() {
  const dispatch = useContext(DispatchCtx);
  return useMemo(
    () => ({
      load: (graph: CanvasLayoutV3) => dispatch({ type: 'load', payload: graph }),
      setNodes: (nodes: CanvasNodeBase[]) => dispatch({ type: 'setNodes', payload: nodes }),
      updateNode: (id: string, patch: Partial<CanvasNodeBase>) =>
        dispatch({ type: 'updateNode', payload: { id, patch } }),
      setEdges: (edges: CanvasEdge[]) => dispatch({ type: 'setEdges', payload: edges }),
      setViewport: (viewport: CanvasViewport) => dispatch({ type: 'setViewport', payload: viewport }),
      setSelection: (ids: string[]) => dispatch({ type: 'setSelection', payload: ids }),
    }),
    [dispatch],
  );
}

// Event handler registration surface for other tracks (grouping, assignment)
type CanvasHandlers = {
  onDropNode?: (payload: { sourceId: string; targetId: string; kind: 'tile' | 'agent' | 'group' }) => void;
  onCreateGroup?: (payload: { memberIds: string[]; name?: string }) => void;
  onAssignAgent?: (payload: { agentId: string; targetIds: string[]; response: BatchAssignmentResponse }) => void;
  onAssignmentError?: (message: string | null) => void;
};

const HandlersCtx = createContext<CanvasHandlers>({});

export function CanvasHandlersProvider({ handlers, children }: { handlers: CanvasHandlers; children: React.ReactNode }) {
  const merged = useMemo(() => handlers, [handlers]);
  return <HandlersCtx.Provider value={merged}>{children}</HandlersCtx.Provider>;
}

export function useCanvasHandlers() {
  return useContext(HandlersCtx);
}

export function useRegisterCanvasHandlers(handlers: CanvasHandlers) {
  // simple convenience wrapper around provider for future extension
  const stableHandlers = useMemo(() => handlers, [handlers]);
  return useCallback(
    ({ children }: { children: React.ReactNode }) => (
      <CanvasHandlersProvider handlers={stableHandlers}>{children}</CanvasHandlersProvider>
    ),
    [stableHandlers],
  );
}
