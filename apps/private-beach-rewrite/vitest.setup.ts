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
