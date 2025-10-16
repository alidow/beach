/**
 * Thin wrapper around the WASM-backed Argon2 implementation that exposes an async helper
 * for deriving Argon2id hashes with the parameters used by the Rust toolchain.
 *
 * We rely on the bundled WASM asset shipped with `argon2-browser`. The package expects a
 * globally-available path to the `.wasm` binary; the first import wires that up so both the
 * browser runtime and Vitest/Node (with fetch available) can resolve the module without any
 * extra configuration.
 */

const HASH_LEN_BYTES = 32;
const TIME_COST = 3;
const MEMORY_COST_KIB = 64 * 1024;
const PARALLELISM = 1;

export interface DeriveParams {
  passphrase: string | Uint8Array;
  salt: string | Uint8Array;
}

type Argon2Module = {
  hash: (params: {
    pass: string | Uint8Array;
    salt: string | Uint8Array;
    time?: number;
    mem?: number;
    hashLen?: number;
    parallelism?: number;
    type?: number;
  }) => Promise<{ hash: Uint8Array }>;
  ArgonType: { Argon2id: number };
};

let wasmBytesCache: Uint8Array | null = null;

async function loadWasmBytes(): Promise<Uint8Array> {
  if (wasmBytesCache) {
    return wasmBytesCache;
  }
  const wasmUrl = new URL('../../assets/argon2.wasm', import.meta.url);
  const isNodeEnvironment = typeof process !== 'undefined' && !!process.versions?.node;
  if (isNodeEnvironment) {
    const [{ readFile }, { fileURLToPath }, { resolve }] = await Promise.all([
      import('node:fs/promises'),
      import('node:url'),
      import('node:path'),
    ]);
    const filePath =
      wasmUrl.protocol === 'file:'
        ? fileURLToPath(wasmUrl)
        : resolve(process.cwd(), 'src/assets/argon2.wasm');
    const buffer = await readFile(filePath);
    wasmBytesCache = buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
  } else {
    const response = await fetch(wasmUrl.toString());
    if (!response.ok) {
      throw new Error(`failed to load argon2 wasm: ${response.status} ${response.statusText}`);
    }
    const buffer = await response.arrayBuffer();
    wasmBytesCache = new Uint8Array(buffer);
  }
  return wasmBytesCache;
}

const argon2ModulePromise: Promise<Argon2Module> = (async () => {
  const wasmBytes = await loadWasmBytes();
  const globalScope = globalThis as typeof globalThis & {
    Module?: Record<string, unknown>;
    loadArgon2WasmBinary?: () => Promise<Uint8Array>;
  };
  const emscriptenModule = (globalScope.Module ??= {});
  if (typeof (emscriptenModule as { wasmBinary?: ArrayBuffer }).wasmBinary === 'undefined') {
    (emscriptenModule as { wasmBinary?: ArrayBuffer }).wasmBinary = wasmBytes.buffer.slice(
      wasmBytes.byteOffset,
      wasmBytes.byteOffset + wasmBytes.byteLength,
    );
  }
  if (typeof (emscriptenModule as { loadArgon2WasmBinary?: () => Promise<Uint8Array> }).loadArgon2WasmBinary !== 'function') {
    (emscriptenModule as { loadArgon2WasmBinary?: () => Promise<Uint8Array> }).loadArgon2WasmBinary =
      async () => wasmBytes;
  }
  if (typeof globalScope.loadArgon2WasmBinary !== 'function') {
    globalScope.loadArgon2WasmBinary = async () => wasmBytes;
  }
  const module = await import('argon2-browser/dist/argon2-bundled.min.js');
  const resolved = (module.default ?? module) as Argon2Module | undefined;
  if (!resolved) {
    throw new Error('argon2 module failed to load');
  }
  return resolved;
})();

export async function deriveArgon2id(params: DeriveParams): Promise<Uint8Array> {
  const wasmModule = await argon2ModulePromise;
  const result = await wasmModule.hash({
    pass: params.passphrase,
    salt: params.salt,
    time: TIME_COST,
    mem: MEMORY_COST_KIB,
    hashLen: HASH_LEN_BYTES,
    parallelism: PARALLELISM,
    type: wasmModule.ArgonType.Argon2id,
  });

  const { hash } = result;
  if (!(hash instanceof Uint8Array)) {
    throw new Error('argon2 hash returned an unexpected payload');
  }
  if (hash.length !== HASH_LEN_BYTES) {
    throw new Error(`argon2 hash length mismatch: expected ${HASH_LEN_BYTES}, received ${hash.length}`);
  }
  return hash;
}
