import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';

interface TraceFrame {
  kind: string;
  ts: number;
  payload: {
    rowKinds?: string[];
    followTail?: boolean;
    [key: string]: unknown;
  };
}

interface TraceFixture {
  generatedAt: string;
  frameCount: number;
  frames: TraceFrame[];
}

const fixturePath = path.resolve(
  __dirname,
  '../../terminal/__fixtures__/rewrite-tail-session.json',
);

const fixtureExists = fs.existsSync(fixturePath);

(fixtureExists ? describe : describe.skip)(
  'BeachTerminal tail regression capture',
  () => {
    const fixture: TraceFixture = JSON.parse(fs.readFileSync(fixturePath, 'utf8'));

    it('contains a captured blank tail viewport (regression reproduction)', () => {
      const missingViewport = fixture.frames.find(
        (frame) =>
          frame.kind === 'buildLines' &&
          Array.isArray(frame.payload.rowKinds) &&
          frame.payload.rowKinds.length > 0 &&
          frame.payload.rowKinds.every((kind) => kind === 'missing'),
      );
      expect(missingViewport).toBeDefined();
    });
  },
);

(fixtureExists ? describe : describe.skip)(
  'BeachTerminal tail regression diagnostics',
  () => {
    const fixture: TraceFixture = JSON.parse(fs.readFileSync(fixturePath, 'utf8'));

    it('records followTail transitions for analysis', () => {
      const followTailEvents = fixture.frames.filter(
        (frame) => frame.kind === 'setFollowTail',
      );
      expect(followTailEvents.length).toBeGreaterThan(0);
    });
  },
);
