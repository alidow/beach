import type { TerminalViewerState } from '../hooks/terminalViewerTypes';
import {
  buildViewerStateFromTerminalDiff,
  extractTerminalStateDiff,
  makeDiffFromLines,
  type TerminalStateDiff,
  type TerminalFramePayload,
} from '../lib/terminalHydrator';

type StaticTerminalInput =
  | readonly string[]
  | TerminalStateDiff
  | TerminalFramePayload
  | {
      payload: TerminalFramePayload;
      sequence?: number;
    };

function normaliseInput(input: StaticTerminalInput): TerminalStateDiff {
  if (Array.isArray(input)) {
    return makeDiffFromLines(input);
  }
  if (
    input &&
    typeof input === 'object' &&
    !Array.isArray(input) &&
    'payload' in input &&
    input.payload &&
    typeof input.payload === 'object'
  ) {
    const sequence =
      typeof (input as { sequence?: unknown }).sequence === 'number'
        ? (input as { sequence: number }).sequence
        : 0;
    return {
      sequence,
      payload: (input as { payload: TerminalFramePayload }).payload,
    };
  }
  if (input && typeof input === 'object' && (input as TerminalFramePayload).type === 'terminal_full') {
    return {
      sequence: 0,
      payload: input as TerminalFramePayload,
    };
  }
  const extracted = extractTerminalStateDiff(input);
  if (extracted) {
    return extracted;
  }
  return makeDiffFromLines([]);
}

export function createStaticTerminalViewer(
  input: StaticTerminalInput,
  options: { viewportRows?: number } = {},
): TerminalViewerState {
  const diff = normaliseInput(input);
  const viewer =
    buildViewerStateFromTerminalDiff(diff, {
      viewportRows: options.viewportRows,
    }) ?? buildViewerStateFromTerminalDiff(makeDiffFromLines([]));

  if (!viewer) {
    throw new Error('Failed to build static terminal viewer');
  }

  return viewer;
}
