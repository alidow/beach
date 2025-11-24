import '@testing-library/jest-dom';

if (typeof globalThis.requestAnimationFrame === 'undefined') {
  globalThis.requestAnimationFrame = (cb: FrameRequestCallback): number =>
    setTimeout(() => cb(Date.now()), 16) as unknown as number;
}

if (typeof globalThis.cancelAnimationFrame === 'undefined') {
  globalThis.cancelAnimationFrame = (handle: number): void => {
    clearTimeout(handle);
  };
}

if (typeof globalThis.ResizeObserver === 'undefined') {
  class ResizeObserver {
    private callback: ResizeObserverCallback;

    constructor(callback: ResizeObserverCallback) {
      this.callback = callback;
    }

    observe(target: Element) {
      const rect = target.getBoundingClientRect();
      this.callback([{ target, contentRect: rect } as ResizeObserverEntry], this);
    }

    unobserve() {
      // no-op
    }

    disconnect() {
      // no-op
    }
  }

  globalThis.ResizeObserver = ResizeObserver;
}

const originalFetch = globalThis.fetch;
if (typeof originalFetch === 'function') {
  globalThis.fetch = (async (...args: Parameters<typeof originalFetch>) => {
    const [input, init] = args;
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input?.toString?.() ?? '';
    if (url && url.includes('/wasm/argon2.wasm')) {
      return new Response(new Uint8Array([0, 1, 2]), { status: 200 });
    }
    return originalFetch(input as any, init as any);
  }) as typeof fetch;
}
