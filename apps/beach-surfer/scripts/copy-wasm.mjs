#!/usr/bin/env node

import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const __filename = fileURLToPath(import.meta.url);
const projectRoot = path.resolve(path.dirname(__filename), '..');
const source = path.join(projectRoot, 'src', 'assets', 'argon2.wasm');
const destinationDir = path.join(projectRoot, 'public', 'wasm');
const destination = path.join(destinationDir, 'argon2.wasm');

async function ensureDirectory(dir) {
  await mkdir(dir, { recursive: true });
}

async function filesMatch(a, b) {
  if (a.length !== b.length) {
    return false;
  }
  return Buffer.compare(a, b) === 0;
}

async function run() {
  const sourceBytes = await readFile(source).catch((error) => {
    throw new Error(
      `Failed to read Argon2 source asset at ${source}: ${error instanceof Error ? error.message : error}`,
    );
  });

  await ensureDirectory(destinationDir);

  let upToDate = false;

  try {
    const existingBytes = await readFile(destination);
    upToDate = await filesMatch(sourceBytes, existingBytes);
  } catch (error) {
    if (!(error && typeof error === 'object' && 'code' in error && error.code === 'ENOENT')) {
      throw error;
    }
  }

  if (upToDate) {
    console.log('[copy-wasm] public/wasm/argon2.wasm is already up to date');
    return;
  }

  await writeFile(destination, sourceBytes);
  console.log('[copy-wasm] synced src/assets/argon2.wasm -> public/wasm/argon2.wasm');
}

run().catch((error) => {
  console.error('[copy-wasm] failed:', error);
  process.exitCode = 1;
});
