#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { existsSync, watch } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const nextDir = path.join(projectRoot, '.next');
const fallbackPath = path.join(nextDir, 'fallback-build-manifest.json');

const fallbackContents = JSON.stringify(
  {
    polyfillFiles: ['static/chunks/polyfills.js'],
    devFiles: [],
    ampDevFiles: [],
    lowPriorityFiles: [
      'static/development/_buildManifest.js',
      'static/development/_ssgManifest.js',
    ],
    rootMainFiles: [],
    pages: {
      '/_app': [],
      '/_error': [],
    },
    ampFirstPages: [],
  },
  null,
  0,
);

async function ensureFallback() {
  await mkdir(nextDir, { recursive: true });
  await writeFile(fallbackPath, fallbackContents);
}

await ensureFallback();

watch(nextDir, { persistent: false }, async (eventType, filename) => {
  if (filename === 'fallback-build-manifest.json' && !existsSync(fallbackPath)) {
    try {
      await ensureFallback();
    } catch (error) {
      console.warn('[dev-with-fallback] failed to recreate fallback manifest', error);
    }
  }
});

const args = process.argv.slice(2);
const child = spawn('next', ['dev', ...args], {
  stdio: 'inherit',
  env: process.env,
});

child.on('exit', (code) => {
  process.exit(code ?? 0);
});
