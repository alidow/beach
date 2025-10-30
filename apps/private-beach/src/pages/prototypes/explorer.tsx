import Head from 'next/head';
import { useCallback, useMemo, useRef, useState } from 'react';
import { Badge } from '../../components/ui/badge';
import { Button } from '../../components/ui/button';
import { Input } from '../../components/ui/input';

type NodeStatus = 'active' | 'paused' | 'warning' | 'error' | 'offline';
type ExplorerNodeType = 'agent' | 'application' | 'observer' | 'archived';

type ExplorerNodeMeta = {
  region?: string;
  controllers?: number;
  cadence?: string;
  childrenCount?: number;
  updatedMinutes?: number;
  tags?: string[];
  description?: string;
  owner?: string;
  offlineSince?: string;
};

type ExplorerNode = {
  id: string;
  label: string;
  type: ExplorerNodeType;
  status?: NodeStatus;
  meta?: ExplorerNodeMeta;
  children?: ExplorerNode[];
};

type ExplorerSection = {
  id: string;
  label: string;
  icon: string;
  description?: string;
  nodes: ExplorerNode[];
};

type ExplorerBucket = {
  id: string;
  label: string | null;
  nodes: ExplorerNode[];
};

const CODEWORDS = [
  'Alpha',
  'Bravo',
  'Charlie',
  'Delta',
  'Echo',
  'Foxtrot',
  'Golf',
  'Hotel',
  'India',
  'Juliet',
  'Kilo',
  'Lima',
  'Mike',
  'November',
  'Oscar',
  'Papa',
  'Quebec',
  'Romeo',
  'Sierra',
  'Tango',
  'Uniform',
  'Victor',
  'Whiskey',
  'Xray',
  'Yankee',
  'Zulu',
] as const;

const TAG_OPTIONS = ['demo', 'analytics', 'ml', 'sre', 'ops', 'sandbox', 'p0', 'observability', 'release'];
const REGION_OPTIONS = ['us-west-2', 'us-east-1', 'eu-central-1', 'ap-southeast-1'];
const OWNER_NAMES = ['A. Chen', 'M. Patel', 'E. Gomez', 'R. Ibarra', 'S. Morgan', 'T. Nakamura'];
const STATUS_POOL: NodeStatus[] = ['active', 'active', 'active', 'warning', 'paused', 'active', 'error', 'active'];

function pick<T>(items: readonly T[], index: number) {
  return items[index % items.length];
}

function toMinutesLabel(minutes: number | undefined) {
  if (typeof minutes !== 'number') return 'Unknown';
  if (minutes < 1) return 'Just now';
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  const remainder = minutes % 60;
  if (hours < 24) {
    return remainder === 0 ? `${hours}h ago` : `${hours}h ${remainder}m ago`;
  }
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

function createApplicationNodes(count: number): ExplorerNode[] {
  return Array.from({ length: count }, (_, index) => {
    const label = `App ${pick(CODEWORDS, index)}-${Math.floor(index / CODEWORDS.length) + 1}`;
    const controllers = index % 4;
    const updatedMinutes = (index * 7) % 240;
    const tags = new Set<string>([
      pick(TAG_OPTIONS, index),
      pick(TAG_OPTIONS, index + 5),
      controllers === 0 ? 'unassigned' : pick(TAG_OPTIONS, index + 11),
    ]);
    return {
      id: `app-${index + 1}`,
      label,
      type: 'application',
      status: STATUS_POOL[index % STATUS_POOL.length],
      meta: {
        region: pick(REGION_OPTIONS, index),
        controllers,
        updatedMinutes,
        cadence: `${[5, 10, 15, 30][index % 4]}s`,
        tags: Array.from(tags),
        description: 'Controllable application session',
      },
    };
  });
}

function createAgentNodes(count: number, applications: ExplorerNode[]): ExplorerNode[] {
  return Array.from({ length: count }, (_, index) => {
    const childCount = 3 + (index % 5);
    const start = (index * 11) % applications.length;
    const children: ExplorerNode[] = [];
    for (let i = 0; i < childCount; i += 1) {
      children.push(applications[(start + i) % applications.length]);
    }
    return {
      id: `agent-${index + 1}`,
      label: `Agent ${pick(CODEWORDS, index + 4)}-${Math.floor(index / CODEWORDS.length) + 1}`,
      type: 'agent',
      status: index % 9 === 0 ? 'warning' : index % 7 === 0 ? 'error' : 'active',
      meta: {
        region: pick(REGION_OPTIONS, index + 1),
        cadence: `${[5, 10, 15, 30][(index + 1) % 4]}s`,
        childrenCount: childCount,
        updatedMinutes: (index * 5) % 120,
        tags:
          index % 3 === 0 ? ['automation', 'demo'] : index % 3 === 1 ? ['sre', 'protected'] : ['infra'],
        description: 'Automation controller session',
      },
      children,
    };
  });
}

function createObserverNodes(count: number): ExplorerNode[] {
  return Array.from({ length: count }, (_, index) => ({
    id: `observer-${index + 1}`,
    label: `Observer ${pick(CODEWORDS, index + 8)}-${Math.floor(index / CODEWORDS.length) + 1}`,
    type: 'observer',
    status: index % 6 === 0 ? 'paused' : 'active',
    meta: {
      region: pick(REGION_OPTIONS, index + 2),
      updatedMinutes: (index * 13) % 200,
      owner: pick(OWNER_NAMES, index),
      tags: index % 6 === 0 ? ['follow-only'] : ['monitor'],
      description: 'Read-only observer receiving streamed updates',
    },
  }));
}

function createArchivedNodes(count: number): ExplorerNode[] {
  return Array.from({ length: count }, (_, index) => ({
    id: `archived-${index + 1}`,
    label: `Archive ${pick(CODEWORDS, index + 12)}-${Math.floor(index / CODEWORDS.length) + 1}`,
    type: 'archived',
    status: 'offline',
    meta: {
      region: pick(REGION_OPTIONS, index + 3),
      offlineSince: `${(index % 14) + 3} days`,
      tags: ['archived'],
      description: 'Inactive session retained for auditability',
    },
  }));
}

const APPLICATION_NODES = createApplicationNodes(240);
const AGENT_NODES = createAgentNodes(96, APPLICATION_NODES);
const OBSERVER_NODES = createObserverNodes(42);
const ARCHIVED_NODES = createArchivedNodes(18);

const COLLECTION_SECTIONS: ExplorerSection[] = [
  {
    id: 'agents',
    label: 'Agents',
    icon: '🤖',
    description: 'Automation controllers capable of orchestrating sessions',
    nodes: AGENT_NODES,
  },
  {
    id: 'applications',
    label: 'Applications',
    icon: '🖥️',
    description: 'Controllable sessions (CLI, GUI, other agents)',
    nodes: APPLICATION_NODES,
  },
  {
    id: 'observers',
    label: 'Observers',
    icon: '👀',
    description: 'Read-only watchers receiving streamed updates',
    nodes: OBSERVER_NODES,
  },
  {
    id: 'archived',
    label: 'Archived',
    icon: '📦',
    description: 'Ended sessions retained for auditability',
    nodes: ARCHIVED_NODES,
  },
];

function collectNodes(sections: ExplorerSection[]): Map<string, ExplorerNode> {
  const catalog = new Map<string, ExplorerNode>();
  const visit = (nodes: ExplorerNode[]) => {
    nodes.forEach((node) => {
      if (!catalog.has(node.id)) {
        catalog.set(node.id, node);
      }
      if (node.children?.length) {
        visit(node.children);
      }
    });
  };
  sections.forEach((section) => visit(section.nodes));
  return catalog;
}

const NODE_CATALOG = collectNodes(COLLECTION_SECTIONS);

function filterNodes(nodes: ExplorerNode[], term: string): ExplorerNode[] {
  if (!term) return nodes;
  const needle = term.toLowerCase();
  return nodes
    .map((node) => {
      const tags = node.meta?.tags ?? [];
      const matchesSelf =
        node.label.toLowerCase().includes(needle) ||
        Boolean(node.meta?.region && node.meta.region.toLowerCase().includes(needle)) ||
        Boolean(node.meta?.description && node.meta.description.toLowerCase().includes(needle)) ||
        tags.some((tag) => tag.toLowerCase().includes(needle));
      const filteredChildren = node.children ? filterNodes(node.children, term) : [];
      if (matchesSelf || filteredChildren.length > 0) {
        return {
          ...node,
          children: node.children ? filteredChildren : undefined,
        };
      }
      return null;
    })
    .filter((value): value is ExplorerNode => Boolean(value));
}

function bucketizeNodes(nodes: ExplorerNode[]): ExplorerBucket[] {
  if (nodes.length <= 35) {
    return [{ id: 'all', label: null, nodes }];
  }

  const buckets = new Map<string, ExplorerNode[]>();
  nodes.forEach((node) => {
    const first = node.label.charAt(0).toUpperCase();
    const key = /^[A-Z]$/.test(first) ? first : '#';
    if (!buckets.has(key)) {
      buckets.set(key, []);
    }
    buckets.get(key)?.push(node);
  });

  return Array.from(buckets.entries())
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([key, bucketNodes]) => ({
      id: `bucket-${key}`,
      label: key === '#' ? 'Other' : key,
      nodes: bucketNodes,
    }));
}

function getNodeIcon(type: ExplorerNodeType) {
  switch (type) {
    case 'agent':
      return '🤖';
    case 'application':
      return '🖥️';
    case 'observer':
      return '👀';
    case 'archived':
      return '📦';
    default:
      return '•';
  }
}

function getStatusBadge(status?: NodeStatus) {
  if (!status) return null;
  switch (status) {
    case 'active':
      return { label: 'Active', variant: 'success' as const };
    case 'paused':
      return { label: 'Paused', variant: 'muted' as const };
    case 'warning':
      return { label: 'Warning', variant: 'warning' as const };
    case 'error':
      return { label: 'Error', variant: 'danger' as const };
    case 'offline':
      return { label: 'Offline', variant: 'muted' as const };
    default:
      return null;
  }
}

function getTypeLabel(type: ExplorerNodeType) {
  switch (type) {
    case 'agent':
      return 'Agent';
    case 'application':
      return 'Application';
    case 'observer':
      return 'Observer';
    case 'archived':
      return 'Archived Session';
    default:
      return 'Item';
  }
}

function formatNodeDescription(node: ExplorerNode) {
  if (node.meta?.description) return node.meta.description;
  switch (node.type) {
    case 'agent':
      return 'Automation controller session';
    case 'application':
      return 'Controllable application session';
    case 'observer':
      return 'Read-only observer session';
    case 'archived':
      return 'Inactive session retained for audit';
    default:
      return 'Explorer item';
  }
}

function countByType(nodes: ExplorerNode[]) {
  return nodes.reduce<Record<ExplorerNodeType, number>>((acc, node) => {
    acc[node.type] = (acc[node.type] ?? 0) + 1;
    return acc;
  }, {} as Record<ExplorerNodeType, number>);
}

function useSelectedNodes(ids: Set<string>) {
  return useMemo(() => {
    const nodes: ExplorerNode[] = [];
    ids.forEach((id) => {
      const match = NODE_CATALOG.get(id);
      if (match) nodes.push(match);
    });
    return nodes;
  }, [ids]);
}

export default function ExplorerPrototypePage() {
  const [searchTerm, setSearchTerm] = useState('');
  const [expandedNodes, setExpandedNodes] = useState<Set<string>>(() => {
    const seed = new Set<string>();
    AGENT_NODES.slice(0, 3).forEach((node) => seed.add(node.id));
    return seed;
  });
  const [selectedIds, setSelectedIds] = useState<Set<string>>(() => new Set());
  const lastAnchorRef = useRef<string | null>(null);
  const [draggedNodeId, setDraggedNodeId] = useState<string | null>(null);
  const [dropTargetId, setDropTargetId] = useState<string | null>(null);

  const filteredSections = useMemo(() => {
    const trimmedTerm = searchTerm.trim();
    return COLLECTION_SECTIONS
      .map((section) => ({
        ...section,
        nodes: filterNodes(section.nodes, trimmedTerm),
      }))
      .filter((section) => section.nodes.length > 0);
  }, [searchTerm]);

  const orderedNodeIds = useMemo(() => {
    const ids: string[] = [];
    filteredSections.forEach((section) => {
      const buckets = bucketizeNodes(section.nodes);
      buckets.forEach((bucket) => {
        bucket.nodes.forEach((node) => {
          if (!ids.includes(node.id)) {
            ids.push(node.id);
          }
          if (node.children?.length && expandedNodes.has(node.id)) {
            node.children.forEach((child) => {
              if (!ids.includes(child.id)) {
                ids.push(child.id);
              }
            });
          }
        });
      });
    });
    return ids;
  }, [filteredSections, expandedNodes]);

  const selectedNodes = useSelectedNodes(selectedIds);
  const selectedCount = selectedIds.size;
  const draggingNode = useMemo(
    () => (draggedNodeId ? NODE_CATALOG.get(draggedNodeId) ?? null : null),
    [draggedNodeId],
  );

  const toggleExpansion = useCallback((nodeId: string) => {
    setExpandedNodes((prev) => {
      const next = new Set(prev);
      if (next.has(nodeId)) {
        next.delete(nodeId);
      } else {
        next.add(nodeId);
      }
      return next;
    });
  }, []);

  const toggleCheckbox = useCallback((nodeId: string, checked: boolean) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (checked) {
        next.add(nodeId);
      } else {
        next.delete(nodeId);
      }
      return next;
    });
    lastAnchorRef.current = nodeId;
  }, []);

  const handleRowActivation = useCallback(
    (event: React.MouseEvent, nodeId: string) => {
      setSelectedIds((prev) => {
        const next = new Set(prev);
        const anchorId = lastAnchorRef.current ?? nodeId;
        if (event.shiftKey && orderedNodeIds.length > 0) {
          const start = orderedNodeIds.indexOf(anchorId);
          const end = orderedNodeIds.indexOf(nodeId);
          if (start !== -1 && end !== -1) {
            const [from, to] = start < end ? [start, end] : [end, start];
            for (let index = from; index <= to; index += 1) {
              next.add(orderedNodeIds[index]);
            }
          } else {
            next.add(nodeId);
          }
        } else if (event.metaKey || event.ctrlKey) {
          if (next.has(nodeId)) {
            next.delete(nodeId);
          } else {
            next.add(nodeId);
          }
        } else {
          next.clear();
          next.add(nodeId);
        }
        return next;
      });
      lastAnchorRef.current = nodeId;
    },
    [orderedNodeIds],
  );

  const clearSelection = useCallback(() => {
    setSelectedIds(new Set());
    lastAnchorRef.current = null;
  }, []);

  const typeCounts = useMemo(() => countByType(selectedNodes), [selectedNodes]);

  return (
    <>
      <Head>
        <title>Explorer Prototype • Private Beach</title>
      </Head>
      <main className="min-h-screen bg-slate-950 text-slate-100">
        <div className="mx-auto flex max-w-6xl flex-col gap-8 px-6 py-10">
          <header className="flex flex-col gap-2">
            <span className="text-xs uppercase tracking-[0.3em] text-slate-500">Prototype</span>
            <h1 className="text-3xl font-semibold text-white sm:text-4xl">Explorer Prototype</h1>
            <p className="max-w-3xl text-sm text-slate-300 sm:text-base">
              Explorer layout focused on agents and applications at scale. Drag sessions onto agents to demo assignment
              affordances, use multi-select for bulk flows, and inspect metadata without leaving the grid.
            </p>
          </header>
          <div className="grid gap-6 lg:grid-cols-[420px_minmax(0,1fr)]">
            <aside className="flex h-[78vh] flex-col overflow-hidden rounded-2xl border border-white/10 bg-slate-900/70 backdrop-blur">
              <div className="border-b border-white/10 px-5 py-4">
                <div className="flex items-center justify-between">
                  <div>
                    <h2 className="text-lg font-semibold">Explorer</h2>
                    <p className="text-xs text-slate-400">Bulk actions, assignment, and inline status previews.</p>
                  </div>
                  <Badge variant="muted">
                    {filteredSections.reduce((acc, section) => acc + section.nodes.length, 0)} items
                  </Badge>
                </div>
              </div>
              {selectedCount > 0 && (
                <div className="flex flex-col gap-2 border-b border-white/5 bg-primary/10 px-5 py-3 text-xs uppercase tracking-wide text-primary-foreground">
                  <div className="flex items-center gap-3 text-primary-foreground/80">
                    <span className="font-semibold">{selectedCount} selected</span>
                    <span className="hidden sm:inline-flex text-primary-foreground/60">
                      Hold Shift for range, Cmd/Ctrl for multi-select
                    </span>
                    <button
                      type="button"
                      className="ml-auto text-[11px] font-medium uppercase tracking-wide text-primary-foreground/70 hover:text-primary-foreground"
                      onClick={clearSelection}
                    >
                      Clear
                    </button>
                  </div>
                  <div className="flex flex-wrap gap-2">
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => {
                        console.info('[prototype] Assign action triggered', Array.from(selectedIds));
                      }}
                    >
                      Assign to…
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => {
                        console.info('[prototype] Pin selection', Array.from(selectedIds));
                      }}
                    >
                      Pin selection
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => {
                        console.info('[prototype] View history', Array.from(selectedIds));
                      }}
                    >
                      View history
                    </Button>
                  </div>
                </div>
              )}
              <div className="space-y-4 px-5 py-4">
                <Input
                  value={searchTerm}
                  onChange={(event) => setSearchTerm(event.target.value)}
                  placeholder="Search by alias, region, or tag…"
                  className="bg-slate-950/40 text-slate-100 placeholder:text-slate-500"
                />
              </div>
              <div className="flex-1 overflow-y-auto px-5 pb-6">
                {filteredSections.length === 0 ? (
                  <div className="mt-12 flex flex-col items-center justify-center text-center text-sm text-slate-400">
                    <span className="text-4xl">🔍</span>
                    <p className="mt-3 font-medium">No matches</p>
                    <p className="max-w-xs text-xs text-slate-500">
                      Try broadening the search query or clearing filters to view more sessions.
                    </p>
                  </div>
                ) : (
                  filteredSections.map((section) => {
                    const buckets = bucketizeNodes(section.nodes);
                    return (
                      <section key={section.id} className="mb-8 last:mb-0">
                        <header className="flex items-center justify-between text-[11px] uppercase tracking-wide text-slate-400">
                          <span className="flex items-center gap-2">
                            <span className="text-lg">{section.icon}</span>
                            {section.label}
                          </span>
                          <span>{section.nodes.length}</span>
                        </header>
                        {section.description && (
                          <p className="mt-1 text-xs text-slate-500">{section.description}</p>
                        )}
                        <div className="mt-3 space-y-4">
                          {buckets.map((bucket) => (
                            <div key={bucket.id} className="space-y-1">
                              {bucket.label && (
                                <div className="px-2 text-[11px] uppercase tracking-wide text-slate-500">
                                  {bucket.label}
                                </div>
                              )}
                              <ul role="tree" className="space-y-1">
                                {bucket.nodes.map((node) => {
                                  const isSelected = selectedIds.has(node.id);
                                  const isExpanded = expandedNodes.has(node.id);
                                  const statusBadge = getStatusBadge(node.status);
                                  const isAgentNode = node.type === 'agent';
                                  const isDraggable = node.type === 'agent' || node.type === 'application';
                                  const isDraggingSelf = draggedNodeId === node.id;
                                  const canAcceptDrop =
                                    isAgentNode &&
                                    Boolean(
                                      draggingNode &&
                                        draggingNode.id !== node.id &&
                                        (draggingNode.type === 'agent' || draggingNode.type === 'application'),
                                    );
                                  const isActiveDropTarget = isAgentNode && dropTargetId === node.id;
                                  const dropHighlightClass = isActiveDropTarget
                                    ? ' border-cyan-300/90 bg-cyan-400/20 shadow-[0_0_36px_rgba(103,232,249,0.45)] animate-pulse'
                                    : canAcceptDrop
                                    ? ' border-cyan-400/40'
                                    : '';
                                  const containerClasses = `group relative flex cursor-pointer select-none items-start gap-3 rounded-xl border border-transparent px-3 py-2 transition hover:border-primary/50 hover:bg-white/5${
                                    isSelected ? ' border-primary/70 bg-primary/15' : ''
                                  }${dropHighlightClass}${isDraggingSelf ? ' opacity-60' : ''}`;
                                  return (
                                    <li key={node.id} role="treeitem" aria-selected={isSelected}>
                                      <div
                                        className={containerClasses}
                                        onClick={(event) => handleRowActivation(event, node.id)}
                                        draggable={isDraggable}
                                        onDragStart={(event) => {
                                          if (!isDraggable) return;
                                          setDraggedNodeId(node.id);
                                          setDropTargetId(null);
                                          event.dataTransfer.effectAllowed = 'move';
                                          event.dataTransfer.setData('application/x-private-beach-node', node.id);
                                          event.dataTransfer.setData('text/plain', node.id);
                                        }}
                                        onDragEnd={() => {
                                          setDraggedNodeId(null);
                                          setDropTargetId(null);
                                        }}
                                        onDragEnter={(event) => {
                                          if (!canAcceptDrop) return;
                                          event.preventDefault();
                                          setDropTargetId(node.id);
                                        }}
                                        onDragOver={(event) => {
                                          if (!canAcceptDrop) return;
                                          event.preventDefault();
                                          event.dataTransfer.dropEffect = 'move';
                                          if (dropTargetId !== node.id) {
                                            setDropTargetId(node.id);
                                          }
                                        }}
                                        onDragLeave={(event) => {
                                          if (!isAgentNode) return;
                                          if (dropTargetId !== node.id) return;
                                          const nextTarget = event.relatedTarget as Node | null;
                                          if (nextTarget && event.currentTarget.contains(nextTarget)) return;
                                          setDropTargetId(null);
                                        }}
                                        onDrop={(event) => {
                                          if (!canAcceptDrop) return;
                                          event.preventDefault();
                                          const originId =
                                            event.dataTransfer.getData('application/x-private-beach-node') || draggedNodeId;
                                          if (!originId || originId === node.id) return;
                                          console.info('[prototype] assignment drop', {
                                            from: originId,
                                            to: node.id,
                                          });
                                          setDropTargetId(node.id);
                                          setDraggedNodeId(null);
                                          window.setTimeout(() => {
                                            setDropTargetId((current) => (current === node.id ? null : current));
                                          }, 450);
                                        }}
                                      >
                                        {isActiveDropTarget && (
                                          <>
                                            <div className="pointer-events-none absolute inset-0 rounded-xl border border-cyan-200/60 bg-cyan-300/20 shadow-[0_0_50px_rgba(103,232,249,0.55)] blur-[1.5px]" />
                                            <div className="pointer-events-none absolute inset-x-0 top-2 z-20 mx-auto w-max rounded-full bg-cyan-200/95 px-3 py-1 text-[11px] font-semibold uppercase tracking-wide text-slate-900 shadow-lg">
                                              Drop to assign
                                            </div>
                                          </>
                                        )}
                                        <div className="relative z-10 flex w-full items-start gap-3">
                                          <div className="flex items-center gap-2 pt-[2px]">
                                            {node.children?.length ? (
                                              <button
                                                type="button"
                                                className="flex h-5 w-5 items-center justify-center rounded-md border border-white/20 text-[11px] font-semibold text-slate-300 transition hover:border-primary/60 hover:text-primary"
                                                onClick={(event) => {
                                                  event.stopPropagation();
                                                  toggleExpansion(node.id);
                                                }}
                                                aria-label={isExpanded ? 'Collapse' : 'Expand'}
                                              >
                                                {isExpanded ? '▾' : '▸'}
                                              </button>
                                            ) : (
                                              <span className="h-5 w-5" />
                                            )}
                                            <input
                                              type="checkbox"
                                              checked={isSelected}
                                              onChange={(event) => {
                                                event.stopPropagation();
                                                toggleCheckbox(node.id, event.target.checked);
                                              }}
                                              className={`h-4 w-4 cursor-pointer rounded border border-white/40 bg-transparent transition checked:bg-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/70 ${
                                                isSelected ? 'opacity-100' : 'opacity-0 group-hover:opacity-100'
                                              }`}
                                            />
                                          </div>
                                          <div className="min-w-0 flex-1 space-y-1">
                                            <div className="flex flex-wrap items-center gap-2">
                                              <span className="text-lg">{getNodeIcon(node.type)}</span>
                                              <span className="truncate font-medium text-slate-100">{node.label}</span>
                                              {statusBadge && (
                                                <Badge variant={statusBadge.variant}>{statusBadge.label}</Badge>
                                              )}
                                              {node.meta?.region && (
                                                <Badge variant="muted" className="uppercase tracking-wide">
                                                  {node.meta.region}
                                                </Badge>
                                              )}
                                              {typeof node.meta?.controllers === 'number' && (
                                                <Badge variant="muted">
                                                  {node.meta.controllers} controller{node.meta.controllers === 1 ? '' : 's'}
                                                </Badge>
                                              )}
                                              {typeof node.meta?.childrenCount === 'number' && (
                                                <Badge variant="muted">
                                                  {node.meta.childrenCount} child{node.meta.childrenCount === 1 ? '' : 'ren'}
                                                </Badge>
                                              )}
                                            </div>
                                            <div className="flex flex-wrap items-center gap-2 text-xs text-slate-400">
                                              <span>{formatNodeDescription(node)}</span>
                                              {typeof node.meta?.updatedMinutes === 'number' && (
                                                <span className="text-slate-500">
                                                  • Updated {toMinutesLabel(node.meta?.updatedMinutes)}
                                                </span>
                                              )}
                                              {node.meta?.owner && <span>• Owner {node.meta.owner}</span>}
                                              {node.meta?.offlineSince && <span>• Offline {node.meta.offlineSince}</span>}
                                            </div>
                                            {node.meta?.tags && node.meta.tags.length > 0 && (
                                              <div className="flex flex-wrap gap-1">
                                                {node.meta.tags.map((tag) => (
                                                  <span
                                                    key={tag}
                                                    className="rounded-full bg-slate-800 px-2 py-0.5 text-[10px] uppercase tracking-wide text-slate-300"
                                                  >
                                                    {tag}
                                                  </span>
                                                ))}
                                              </div>
                                            )}
                                          </div>
                                        </div>
                                      </div>
                                      {node.children?.length && isExpanded && (
                                        <ul className="ml-14 mt-1 space-y-1 border-l border-white/10 pl-4">
                                          {node.children.map((child) => {
                                            const childSelected = selectedIds.has(child.id);
                                            const childStatus = getStatusBadge(child.status);
                                            const childDraggable =
                                              child.type === 'application' || child.type === 'agent';
                                            const childDragging = draggedNodeId === child.id;
                                            const childClasses = `group relative flex cursor-pointer select-none items-start gap-3 rounded-xl border border-transparent px-3 py-2 text-sm transition hover:border-primary/40 hover:bg-white/5${
                                              childSelected ? ' border-primary/60 bg-primary/15' : ''
                                            }${childDragging ? ' opacity-60' : ''}`;
                                            return (
                                              <li key={`${node.id}-${child.id}`}>
                                                <div
                                                  className={childClasses}
                                                  onClick={(event) => handleRowActivation(event, child.id)}
                                                  draggable={childDraggable}
                                                  onDragStart={(event) => {
                                                    if (!childDraggable) return;
                                                    event.stopPropagation();
                                                    setDraggedNodeId(child.id);
                                                    setDropTargetId(null);
                                                    event.dataTransfer.effectAllowed = 'move';
                                                    event.dataTransfer.setData(
                                                      'application/x-private-beach-node',
                                                      child.id,
                                                    );
                                                    event.dataTransfer.setData('text/plain', child.id);
                                                  }}
                                                  onDragEnd={() => {
                                                    setDraggedNodeId(null);
                                                    setDropTargetId(null);
                                                  }}
                                                >
                                                  <span className="text-lg">{getNodeIcon(child.type)}</span>
                                                  <div className="min-w-0 flex-1">
                                                    <div className="flex flex-wrap items-center gap-2">
                                                      <span className="truncate font-medium text-slate-100">
                                                        {child.label}
                                                      </span>
                                                      {childStatus && (
                                                        <Badge variant={childStatus.variant}>{childStatus.label}</Badge>
                                                      )}
                                                      {child.meta?.region && (
                                                        <Badge variant="muted" className="uppercase tracking-wide">
                                                          {child.meta.region}
                                                        </Badge>
                                                      )}
                                                    </div>
                                                    <div className="mt-1 text-xs text-slate-400">
                                                      Updated {toMinutesLabel(child.meta?.updatedMinutes)} •{' '}
                                                      {child.meta?.cadence ?? '— cadence'}
                                                    </div>
                                                  </div>
                                                </div>
                                              </li>
                                            );
                                          })}
                                        </ul>
                                      )}
                                    </li>
                                  );
                                })}
                              </ul>
                            </div>
                          ))}
                        </div>
                      </section>
                    );
                  })
                )}
              </div>
            </aside>
            <section className="flex h-[78vh] flex-col justify-between rounded-2xl border border-white/10 bg-slate-900/40 p-6 backdrop-blur">
              <div className="space-y-6 overflow-y-auto">
                <div>
                  <p className="text-xs uppercase tracking-[0.3em] text-slate-500">Selection</p>
                  <h2 className="mt-1 text-2xl font-semibold text-white">
                    {selectedCount > 0 ? `${selectedCount} item${selectedCount === 1 ? '' : 's'} selected` : 'No selection'}
                  </h2>
                </div>
                {selectedNodes.length === 0 ? (
                  <div className="rounded-xl border border-dashed border-white/10 bg-white/5 px-5 py-6 text-sm text-slate-300">
                    <p className="font-medium text-white">Pick a node to inspect metadata.</p>
                    <p className="mt-2 text-slate-400">
                      Selecting an agent reveals its cadence, managed children, and status. Multi-select nodes to
                      simulate bulk flows like “Assign to agent” or “Pin selection”.
                    </p>
                  </div>
                ) : selectedNodes.length === 1 ? (
                  (() => {
                    const node = selectedNodes[0];
                    const badge = getStatusBadge(node.status);
                    return (
                      <div className="space-y-5">
                        <div className="rounded-xl border border-white/10 bg-white/5 p-5">
                          <div className="flex items-start gap-4">
                            <div className="flex h-12 w-12 items-center justify-center rounded-lg bg-primary/20 text-2xl">
                              {getNodeIcon(node.type)}
                            </div>
                            <div className="flex-1 space-y-2">
                              <div className="flex flex-wrap items-center gap-3">
                                <h3 className="text-xl font-semibold text-white">{node.label}</h3>
                                <Badge variant="muted">{getTypeLabel(node.type)}</Badge>
                                {badge && <Badge variant={badge.variant}>{badge.label}</Badge>}
                                {node.meta?.region && (
                                  <Badge variant="muted" className="uppercase tracking-wide">
                                    {node.meta.region}
                                  </Badge>
                                )}
                              </div>
                              <p className="text-sm text-slate-300">{formatNodeDescription(node)}</p>
                              {node.meta?.tags && node.meta.tags.length > 0 && (
                                <div className="flex flex-wrap gap-2 pt-1">
                                  {node.meta.tags.map((tag) => (
                                    <span
                                      key={tag}
                                      className="rounded-full bg-slate-800 px-3 py-1 text-[11px] uppercase tracking-wide text-slate-200"
                                    >
                                      {tag}
                                    </span>
                                  ))}
                                </div>
                              )}
                            </div>
                          </div>
                        </div>
                        <dl className="grid gap-4 text-sm sm:grid-cols-2">
                          {node.meta?.cadence && (
                            <div className="rounded-lg border border-white/10 bg-white/5 p-4">
                              <dt className="text-xs uppercase tracking-wide text-slate-400">Cadence</dt>
                              <dd className="text-base font-semibold text-white">{node.meta.cadence}</dd>
                            </div>
                          )}
                          {typeof node.meta?.childrenCount === 'number' && (
                            <div className="rounded-lg border border-white/10 bg-white/5 p-4">
                              <dt className="text-xs uppercase tracking-wide text-slate-400">Children</dt>
                              <dd className="text-base font-semibold text-white">{node.meta.childrenCount}</dd>
                            </div>
                          )}
                          {typeof node.meta?.controllers === 'number' && (
                            <div className="rounded-lg border border-white/10 bg-white/5 p-4">
                              <dt className="text-xs uppercase tracking-wide text-slate-400">Controllers</dt>
                              <dd className="text-base font-semibold text-white">
                                {node.meta.controllers || 'None'}
                              </dd>
                            </div>
                          )}
                          {typeof node.meta?.updatedMinutes === 'number' && (
                            <div className="rounded-lg border border-white/10 bg-white/5 p-4">
                              <dt className="text-xs uppercase tracking-wide text-slate-400">Last update</dt>
                              <dd className="text-base font-semibold text-white">
                                {toMinutesLabel(node.meta.updatedMinutes)}
                              </dd>
                            </div>
                          )}
                          {node.meta?.owner && (
                            <div className="rounded-lg border border-white/10 bg-white/5 p-4">
                              <dt className="text-xs uppercase tracking-wide text-slate-400">Owner</dt>
                              <dd className="text-base font-semibold text-white">{node.meta.owner}</dd>
                            </div>
                          )}
                          {node.meta?.offlineSince && (
                            <div className="rounded-lg border border-white/10 bg-white/5 p-4">
                              <dt className="text-xs uppercase tracking-wide text-slate-400">Offline since</dt>
                              <dd className="text-base font-semibold text-white">{node.meta.offlineSince}</dd>
                            </div>
                          )}
                        </dl>
                        {node.children?.length ? (
                          <div>
                            <h3 className="text-xs uppercase tracking-wide text-slate-400">Sample children</h3>
                            <ul className="mt-2 space-y-2">
                              {node.children.slice(0, 5).map((child) => (
                                <li
                                  key={child.id}
                                  className="flex items-center justify-between rounded-lg border border-white/5 bg-white/5 px-3 py-2 text-sm"
                                >
                                  <div className="flex items-center gap-2">
                                    <span className="text-lg">{getNodeIcon(child.type)}</span>
                                    <span className="font-medium text-white">{child.label}</span>
                                  </div>
                                  <span className="text-xs text-slate-400">
                                    Updated {toMinutesLabel(child.meta?.updatedMinutes)}
                                  </span>
                                </li>
                              ))}
                              {node.children.length > 5 && (
                                <li className="text-xs text-slate-400">
                                  +{node.children.length - 5} more linked sessions
                                </li>
                              )}
                            </ul>
                          </div>
                        ) : null}
                      </div>
                    );
                  })()
                ) : (
                  <div className="space-y-5">
                    <div className="rounded-xl border border-white/10 bg-white/5 p-5">
                      <h3 className="text-sm font-semibold text-white">Bulk selection summary</h3>
                      <p className="mt-1 text-sm text-slate-300">
                        Multi-select actions can target mixed node types. Counts below help validate bulk flows for the
                        new command toolbar.
                      </p>
                      <div className="mt-4 grid gap-3 text-sm sm:grid-cols-2">
                        {Object.entries(typeCounts)
                          .filter(([, value]) => Boolean(value))
                          .map(([type, count]) => (
                            <div
                              key={type}
                              className="flex items-center justify-between rounded-lg border border-white/10 bg-slate-900/60 px-3 py-2"
                            >
                              <span className="flex items-center gap-2">
                                <span className="text-lg">{getNodeIcon(type as ExplorerNodeType)}</span>
                                {getTypeLabel(type as ExplorerNodeType)}
                              </span>
                              <span className="font-semibold text-white">{count}</span>
                            </div>
                          ))}
                      </div>
                    </div>
                    <div className="flex flex-wrap gap-2">
                      {selectedNodes.slice(0, 8).map((node) => (
                        <span
                          key={`pill-${node.id}`}
                          className="rounded-full border border-white/10 bg-white/10 px-3 py-1 text-xs text-slate-200"
                        >
                          {getNodeIcon(node.type)} {node.label}
                        </span>
                      ))}
                      {selectedNodes.length > 8 && (
                        <span className="rounded-full border border-white/10 bg-white/10 px-3 py-1 text-xs text-slate-300">
                          +{selectedNodes.length - 8} more
                        </span>
                      )}
                    </div>
                    <div className="rounded-xl border border-dashed border-white/15 bg-white/5 px-4 py-3 text-xs text-slate-400">
                      Prototype hint: drop a selection onto an agent in the explorer to simulate “Assign”. The explorer
                      shows the target with a highlighted drop affordance and preview label.
                    </div>
                  </div>
                )}
              </div>
              <footer className="border-t border-white/5 pt-4 text-xs text-slate-500">
                This page lives at{' '}
                <code className="rounded bg-white/10 px-1 py-0.5 text-[11px] text-slate-200">/prototypes/explorer</code>.
                Feedback welcome before we commit to engineering the full experience.
              </footer>
            </section>
          </div>
        </div>
      </main>
    </>
  );
}
