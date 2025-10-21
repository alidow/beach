#!/usr/bin/env node

import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const __filename = fileURLToPath(import.meta.url);
const projectRoot = path.resolve(path.dirname(__filename), '..');
const destinationDir = path.join(projectRoot, 'public', 'wasm');
const require = createRequire(import.meta.url);

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
  const artifacts = [
    {
      label: 'argon2.wasm',
      loadBytes: async () => {
        const sourcePath = path.join(projectRoot, 'src', 'assets', 'argon2.wasm');
        const bytes = await readFile(sourcePath).catch((error) => {
          throw new Error(
            `Failed to read Argon2 source asset at ${sourcePath}: ${
              error instanceof Error ? error.message : error
            }`,
          );
        });
        return { bytes, sourcePath };
      },
    },
    {
      label: 'noise-c.wasm',
      loadBytes: async () => {
        let sourcePath;
        try {
          sourcePath = require.resolve('noise-c.wasm/src/noise-c.wasm');
        } catch (error) {
          throw new Error(
            `Failed to resolve noise-c.wasm package asset: ${
              error instanceof Error ? error.message : error
            }`,
          );
        }
        const bytes = await readFile(sourcePath).catch((error) => {
          throw new Error(
            `Failed to read Noise source asset at ${sourcePath}: ${
              error instanceof Error ? error.message : error
            }`,
          );
        });
        return { bytes, sourcePath };
      },
    },
  ];

  await ensureDirectory(destinationDir);

  for (const artifact of artifacts) {
    const { bytes: sourceBytes, sourcePath } = await artifact.loadBytes();
    const destination = path.join(destinationDir, artifact.label);
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
      console.log(`[copy-wasm] ${artifact.label} already up to date (${destination})`);
      continue;
    }

    await writeFile(destination, sourceBytes);
    console.log(`[copy-wasm] synced ${sourcePath} -> ${destination}`);
  }
}

run().catch((error) => {
  console.error('[copy-wasm] failed:', error);
  process.exitCode = 1;
});
