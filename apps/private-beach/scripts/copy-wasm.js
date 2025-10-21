#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');

const projectRoot = path.join(__dirname, '..');
const publicWasmDir = path.join(projectRoot, 'public', 'wasm');

const ensureDir = (dir) => {
  fs.mkdirSync(dir, { recursive: true });
};

const copyFile = (source, destination) => {
  fs.copyFileSync(source, destination);
  console.log(`[copy-wasm] copied ${source} -> ${destination}`);
};

const run = () => {
  ensureDir(publicWasmDir);

  const noiseSource = require.resolve('noise-c.wasm/src/noise-c.wasm');
  const noiseDestination = path.join(publicWasmDir, 'noise-c.wasm');
  copyFile(noiseSource, noiseDestination);

  const argonSource = path.join(
    projectRoot,
    '..',
    'beach-surfer',
    'src',
    'assets',
    'argon2.wasm',
  );
  if (!fs.existsSync(argonSource)) {
    throw new Error(`argon2 source not found at ${argonSource}`);
  }
  const argonDestination = path.join(publicWasmDir, 'argon2.wasm');
  copyFile(argonSource, argonDestination);
};

run();
