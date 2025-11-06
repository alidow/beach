import '@testing-library/jest-dom/vitest';

class StubResizeObserver implements ResizeObserver {
  readonly [Symbol.toStringTag] = 'ResizeObserver';
  // eslint-disable-next-line @typescript-eslint/no-useless-constructor
  constructor(private readonly _callback: ResizeObserverCallback) {}
  observe(_target: Element, _options?: ResizeObserverOptions): void {}
  unobserve(_target: Element): void {}
  disconnect(): void {}
  takeRecords(): ResizeObserverEntry[] {
    return [];
  }
}

if (typeof globalThis.ResizeObserver === 'undefined') {
  // @ts-expect-error assign stub for jsdom environment
  globalThis.ResizeObserver = StubResizeObserver;
}

const wasmMockBytes = new Uint8Array([0x00, 0x61, 0x73, 0x6d]); // "\0asm" magic header
const wasmResponse = new Response(wasmMockBytes.buffer, {
  status: 200,
  headers: {
    'Content-Type': 'application/wasm',
  },
});

const originalFetch = typeof globalThis.fetch === 'function' ? globalThis.fetch.bind(globalThis) : undefined;

// jsdom exposes a browser-like environment, so BeachTerminal pulls in crypto modules that attempt
// to fetch wasm binaries. Provide a deterministic stub so unit tests do not rely on the filesystem.
globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
  const url =
    typeof input === 'string'
      ? input
      : input instanceof Request
        ? input.url
        : input instanceof URL
          ? input.toString()
          : '';

  if (url.includes('/wasm/argon2.wasm') || url.includes('/wasm/noise-c.wasm')) {
    return wasmResponse.clone();
  }

  if (originalFetch) {
    return originalFetch(input as RequestInfo, init);
  }

  throw new TypeError(`fetch stub: unsupported URL ${url}`);
};
