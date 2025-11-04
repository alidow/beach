import * as React from 'react';
// eslint-disable-next-line import/no-extraneous-dependencies
import ReactCompiled from 'next/dist/compiled/react';

if (typeof (React as unknown as { cache?: unknown }).cache !== 'function') {
  const compiledCache = (ReactCompiled as unknown as { cache?: unknown }).cache;
  if (typeof compiledCache === 'function') {
    (React as unknown as { cache: unknown }).cache = compiledCache;
  }
}
