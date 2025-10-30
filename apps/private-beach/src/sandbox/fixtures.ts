import pongLhsFixture from '../tests/fixtures/pong-lhs-terminal.json';

const FIXTURES: Record<string, readonly string[]> = {
  'pong-lhs': pongLhsFixture as readonly string[],
};

export function resolveTerminalFixture(key: string): readonly string[] | null {
  const normalised = key.trim().toLowerCase();
  return FIXTURES[normalised] ?? null;
}
