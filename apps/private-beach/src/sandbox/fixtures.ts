import pongLhsFixture from '../tests/fixtures/pong-lhs-terminal.json';
import pongLhsStyledFixture from '../tests/fixtures/pong-lhs-terminal-styled.json';
import type { TerminalStateDiff, TerminalFramePayload } from '../lib/terminalHydrator';

type TerminalFixture = readonly string[] | TerminalStateDiff | TerminalFramePayload;

const FIXTURES: Record<string, TerminalFixture> = {
  'pong-lhs': pongLhsFixture as readonly string[],
  'pong-lhs-styled': pongLhsStyledFixture as TerminalStateDiff,
};

export function resolveTerminalFixture(key: string): TerminalFixture | null {
  const normalised = key.trim().toLowerCase();
  return FIXTURES[normalised] ?? null;
}
