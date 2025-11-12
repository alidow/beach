'use client';

import type { Layout as ReactGridLayoutItem } from 'react-grid-layout';
import type { CanvasLayout } from '../canvas';
import type { BeachLayoutItem } from '../lib/api';
import {
  applyGridMetadataToLayout,
  beachItemsToGridSnapshot,
  extractGridLayoutSnapshot,
  reactGridToGridSnapshot,
  withLayoutDashboardMetadata,
  type GridLayoutSnapshot,
} from './gridLayout';

export type GridCommandResult = {
  layout: CanvasLayout;
  snapshot: GridLayoutSnapshot;
  mutated: boolean;
};

export type GridCommandContext = {
  rowHeightPx?: number;
  layoutVersion?: number;
};

export type ReactGridCommandContext = GridCommandContext & {
  cols: number;
};

function applySnapshot(layout: CanvasLayout, snapshot: GridLayoutSnapshot): GridCommandResult {
  const withDashboard = withLayoutDashboardMetadata(layout, snapshot);
  const updatedLayout = applyGridMetadataToLayout(withDashboard, snapshot.tiles);
  const mutated = !Object.is(updatedLayout, layout);
  return {
    layout: mutated ? updatedLayout : layout,
    snapshot,
    mutated,
  };
}

function applyReactGridCommand(
  layout: CanvasLayout,
  nextLayouts: ReactGridLayoutItem[],
  context: ReactGridCommandContext,
  options?: { manualLayout?: boolean },
): GridCommandResult {
  const previous = extractGridLayoutSnapshot(layout);
  const snapshot = reactGridToGridSnapshot(nextLayouts, {
    cols: context.cols,
    rowHeightPx: context.rowHeightPx,
    layoutVersion: context.layoutVersion,
    previous,
  });
  const manualLayout = options?.manualLayout ?? true;
  for (const metadata of Object.values(snapshot.tiles)) {
    metadata.manualLayout = manualLayout;
  }
  return applySnapshot(layout, snapshot);
}

export function applyGridDragCommand(
  layout: CanvasLayout,
  nextLayouts: ReactGridLayoutItem[],
  context: ReactGridCommandContext,
): GridCommandResult {
  return applyReactGridCommand(layout, nextLayouts, context, { manualLayout: true });
}

export function applyGridResizeCommand(
  layout: CanvasLayout,
  nextLayouts: ReactGridLayoutItem[],
  context: ReactGridCommandContext,
): GridCommandResult {
  return applyReactGridCommand(layout, nextLayouts, context, { manualLayout: true });
}

export function applyGridAutosizeCommand(
  layout: CanvasLayout,
  nextLayouts: ReactGridLayoutItem[],
  context: ReactGridCommandContext,
): GridCommandResult {
  return applyReactGridCommand(layout, nextLayouts, context, { manualLayout: false });
}

export function applyGridPresetCommand(
  layout: CanvasLayout,
  preset: BeachLayoutItem[],
  context?: GridCommandContext & { defaultCols?: number; defaultRowHeightPx?: number },
  options?: { manualLayout?: boolean },
): GridCommandResult {
  const snapshot = beachItemsToGridSnapshot(preset, {
    defaultCols: context?.defaultCols,
    defaultRowHeightPx: context?.defaultRowHeightPx,
    layoutVersion: context?.layoutVersion,
  });
  if (typeof context?.rowHeightPx === 'number') {
    snapshot.rowHeightPx = context.rowHeightPx;
  }
  if (typeof context?.layoutVersion === 'number') {
    snapshot.layoutVersion = context.layoutVersion;
  }
  const manualLayout = options?.manualLayout ?? false;
  for (const metadata of Object.values(snapshot.tiles)) {
    metadata.manualLayout = manualLayout;
  }
  return applySnapshot(layout, snapshot);
}
