declare module 'argon2-browser' {
  export interface Argon2HashParams {
    pass: string | Uint8Array;
    salt: string | Uint8Array;
    time?: number;
    mem?: number;
    hashLen?: number;
    parallelism?: number;
    type?: number;
  }

  export interface Argon2HashResult {
    hash: Uint8Array;
    hashHex: string;
    encoded: string;
  }

  export interface Argon2Module {
    hash(params: Argon2HashParams): Promise<Argon2HashResult>;
    verify(params: Argon2HashParams & { encoded: string }): Promise<Argon2HashResult>;
    ArgonType: {
      Argon2d: number;
      Argon2i: number;
      Argon2id: number;
    };
  }

  const argon2: Argon2Module;
  export default argon2;
}

declare module 'argon2-browser/dist/argon2-bundled.min.js' {
  import type { Argon2Module } from 'argon2-browser';
  const argon2: Argon2Module;
  export default argon2;
}
