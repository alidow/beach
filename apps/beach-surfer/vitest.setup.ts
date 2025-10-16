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
