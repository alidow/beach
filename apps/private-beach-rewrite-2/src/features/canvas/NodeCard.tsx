'use client';

import type { CanvasNodeDefinition } from './types';

type NodeCardContentProps = {
  node: CanvasNodeDefinition;
};

export function NodeCardContent({ node }: NodeCardContentProps) {
  return (
    <>
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm font-medium text-foreground">{node.label}</span>
        <span className="rounded-md bg-secondary px-2 py-0.5 text-[11px] font-medium uppercase tracking-wide text-secondary-foreground">
          {node.nodeType}
        </span>
      </div>
      {node.description ? <p className="mt-2 text-xs leading-relaxed text-muted-foreground">{node.description}</p> : null}
      <dl className="mt-3 grid grid-cols-2 gap-2 text-[11px] leading-4 text-muted-foreground">
        <div>
          <dt className="font-medium text-foreground">Width</dt>
          <dd>{node.defaultSize.width}px</dd>
        </div>
        <div>
          <dt className="font-medium text-foreground">Height</dt>
          <dd>{node.defaultSize.height}px</dd>
        </div>
      </dl>
    </>
  );
}
