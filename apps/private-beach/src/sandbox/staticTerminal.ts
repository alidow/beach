import { TerminalGridStore } from '../../../beach-surfer/src/terminal/gridStore';
import type { TerminalViewerState } from '../hooks/useSessionTerminal';
import type { Update } from '../../../beach-surfer/src/protocol/types';

function packLine(line: string): number[] {
  const chars = Array.from(line);
  return chars.map((char) => {
    const codePoint = char.codePointAt(0) ?? 32;
    return codePoint * 2 ** 32;
  });
}

export function createStaticTerminalViewer(
  lines: string[],
  options: { viewportRows?: number } = {},
): TerminalViewerState {
  const viewportRows = options.viewportRows ?? Math.min(24, Math.max(1, lines.length));
  const maxWidth = lines.reduce((width, line) => Math.max(width, Array.from(line).length), 0) || 1;
  const totalRows = lines.length || viewportRows;
  const store = new TerminalGridStore(maxWidth);

  store.setGridSize(totalRows, maxWidth);
  store.setBaseRow(0);
  store.setFollowTail(true);

  const updates: Update[] = lines.map((line, index) => ({
    type: 'row',
    row: index,
    seq: index + 1,
    cells: packLine(line.padEnd(maxWidth, ' ')),
  }));

  if (updates.length > 0) {
    store.applyUpdates(updates, { authoritative: true, origin: 'sandbox-static' });
  }

  const viewportTop = Math.max(0, totalRows - viewportRows);
  store.setViewport(viewportTop, viewportRows);

  return {
    store,
    transport: null,
    connecting: false,
    error: null,
    status: 'connected',
    secureSummary: null,
    latencyMs: 0,
  };
}
