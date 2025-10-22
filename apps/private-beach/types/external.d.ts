declare module 'argon2-browser' {
  const argon2: any;
  export default argon2;
}

declare module 'argon2-browser/dist/argon2-bundled.min.js' {
  const module: any;
  export default module;
}

declare module 'noise-c.wasm/src/noise-c.wasm' {
  const wasmPath: string;
  export default wasmPath;
}

declare module '*.wasm' {
  const wasm: ArrayBuffer;
  export default wasm;
}

declare module 'next-themes/dist/types' {
  export * from 'next-themes';
}

declare module '../../../../temp/terminal-preview-harness' {
  const harness: unknown;
  export default harness;
}

declare const __APP_VERSION__: string;

interface ImportMeta {
  env: Record<string, string | undefined>;
}
