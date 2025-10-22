import { act, render, screen } from '@testing-library/react';
import { describe, expect, it, beforeEach, afterEach } from 'vitest';
import { useState } from 'react';
import type { ControllerPairing } from '../../lib/api';
import { useControllerPairingStreams } from '../useControllerPairingStreams';

class MockEventSource {
  static instances: MockEventSource[] = [];
  url: string;
  readyState = 0;
  onopen: ((event: Event) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  private listeners: Map<string, Set<(event: MessageEvent<string>) => void>> = new Map();

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
  }

  addEventListener(type: string, listener: EventListenerOrEventListenerObject) {
    if (typeof listener === 'function') {
      if (!this.listeners.has(type)) {
        this.listeners.set(type, new Set());
      }
      this.listeners.get(type)!.add(listener as (event: MessageEvent<string>) => void);
    }
  }

  close() {
    this.readyState = 2;
    this.listeners.clear();
  }

  emit(type: string, payload: unknown) {
    const listeners = this.listeners.get(type);
    if (!listeners) return;
    const event = { data: JSON.stringify(payload) } as MessageEvent<string>;
    listeners.forEach((listener) => listener(event));
  }

  triggerOpen() {
    this.readyState = 1;
    this.onopen?.(new Event('open'));
  }

  triggerError() {
    this.readyState = 2;
    this.onerror?.(new Event('error'));
  }
}

type HarnessProps = {
  managerUrl: string;
  managerToken: string;
  controllerIds: string[];
};

function PairingStreamHarness({ managerUrl, managerToken, controllerIds }: HarnessProps) {
  const [pairings, setPairings] = useState<ControllerPairing[]>([]);
  useControllerPairingStreams({
    managerUrl,
    managerToken,
    controllerSessionIds: controllerIds,
    setPairings,
  });
  return <div data-testid="pairings-state">{JSON.stringify(pairings)}</div>;
}

const originalEventSource = global.EventSource as any;

describe('useControllerPairingStreams', () => {
  beforeEach(() => {
    MockEventSource.instances = [];
    (global as any).EventSource = MockEventSource as any;
  });

  afterEach(() => {
    (global as any).EventSource = originalEventSource;
  });

  it('applies controller pairing events from the SSE stream', async () => {
    render(
      <PairingStreamHarness
        managerUrl="http://localhost:8080"
        managerToken="test-token"
        controllerIds={['controller-1']}
      />,
    );

    expect(MockEventSource.instances).toHaveLength(1);
    const instance = MockEventSource.instances[0];
    expect(instance.url).toContain('/sessions/controller-1/controllers/stream');
    expect(instance.url).toContain('access_token=test-token');

    await act(async () => {
      instance.emit('controller_pairing', {
        action: 'added',
        controller_session_id: 'controller-1',
        child_session_id: 'child-1',
        pairing: {
          pairing_id: 'pair-1',
          controller_session_id: 'controller-1',
          child_session_id: 'child-1',
          update_cadence: 'fast',
          prompt_template: 'Drive with precision',
          transport_status: { transport: 'fast_path', latency_ms: 42, last_event_ms: 1234 },
          created_at_ms: 1111,
          updated_at_ms: 2222,
        },
      });
    });

    const afterAdd = JSON.parse(screen.getByTestId('pairings-state').textContent ?? '[]');
    expect(afterAdd).toHaveLength(1);
    expect(afterAdd[0]).toMatchObject({
      controller_session_id: 'controller-1',
      child_session_id: 'child-1',
      update_cadence: 'fast',
      transport_status: { transport: 'fast_path', latency_ms: 42, last_event_ms: 1234 },
    });

    await act(async () => {
      instance.emit('controller_pairing', {
        action: 'removed',
        controller_session_id: 'controller-1',
        child_session_id: 'child-1',
      });
    });

    const afterRemove = JSON.parse(screen.getByTestId('pairings-state').textContent ?? '[]');
    expect(afterRemove).toHaveLength(0);
  });
});
