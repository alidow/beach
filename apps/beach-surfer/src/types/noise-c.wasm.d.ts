declare module 'noise-c.wasm' {
  interface NoiseConstants {
    NOISE_ROLE_INITIATOR: number;
    NOISE_ROLE_RESPONDER: number;
    NOISE_ACTION_NONE: number;
    NOISE_ACTION_WRITE_MESSAGE: number;
    NOISE_ACTION_READ_MESSAGE: number;
    NOISE_ACTION_FAILED: number;
    NOISE_ACTION_SPLIT: number;
    NOISE_DH_CURVE25519: number;
    NOISE_DH_CURVE448: number;
  }

  interface NoiseCipherState {
    free(): void;
  }

  interface NoiseHandshakeState {
    Initialize(
      prologue: Uint8Array | null,
      s: Uint8Array | null,
      rs: Uint8Array | null,
      psk: Uint8Array | null,
    ): void;
    GetAction(): number;
    WriteMessage(payload: Uint8Array | null): Uint8Array;
    ReadMessage(message: Uint8Array, payloadNeeded?: boolean, fallbackSupported?: boolean): Uint8Array | null;
    Split(): [NoiseCipherState, NoiseCipherState];
    GetHandshakeHash(): Uint8Array;
    free(): void;
  }

  interface NoiseModule {
    constants: NoiseConstants;
    HandshakeState: new (protocolName: string, role: number) => NoiseHandshakeState;
    CreateKeyPair(curveId: number): [Uint8Array, Uint8Array];
  }

  interface NoiseInitOptions {
    locateFile?: (path: string) => string;
    wasmBinary?: Uint8Array;
  }

  type CreateNoiseModule = {
    (callback: (module: NoiseModule) => void): void;
    (options: NoiseInitOptions, callback: (module: NoiseModule) => void): void;
  };

  const createNoiseModule: CreateNoiseModule;
  export default createNoiseModule;
}
