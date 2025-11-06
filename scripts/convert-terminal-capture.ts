#!/usr/bin/env ts-node

/**
 * Converts a captured BeachTerminal trace (produced by window.__BEACH_TRACE_DUMP__())
 * into a fixture JSON we can replay inside unit tests.
 *
 * Usage:
 *   pnpm ts-node scripts/convert-terminal-capture.ts capture.json apps/beach-surfer/src/terminal/__fixtures__/rewrite-tail-session.json
 */

import fs from 'node:fs';
import path from 'node:path';

type TraceFrame = {
  kind: string;
  ts: number;
  payload: unknown;
};

function readInput(filePath: string): TraceFrame[] {
  const raw = fs.readFileSync(filePath, 'utf8');
  return JSON.parse(raw) as TraceFrame[];
}

function writeFixture(frames: TraceFrame[], outputPath: string): void {
  const dir = path.dirname(outputPath);
  fs.mkdirSync(dir, { recursive: true });
  const fixture = {
    generatedAt: new Date().toISOString(),
    frameCount: frames.length,
    frames,
  };
  fs.writeFileSync(outputPath, JSON.stringify(fixture, null, 2));
  // eslint-disable-next-line no-console
  console.log(`Wrote ${frames.length} frames to ${outputPath}`);
}

function main(): void {
  const [inputPath, outputPath = 'apps/beach-surfer/src/terminal/__fixtures__/trace.json'] = process.argv.slice(2);
  if (!inputPath) {
    // eslint-disable-next-line no-console
    console.error('Usage: pnpm ts-node scripts/convert-terminal-capture.ts <capture.json> [output.json]');
    process.exit(1);
  }
  const frames = readInput(inputPath);
  writeFixture(frames, outputPath);
}

main();
