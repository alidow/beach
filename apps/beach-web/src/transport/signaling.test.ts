import { describe, expect, it } from 'vitest';
import { SignalingClient, type ServerMessage } from './signaling';

describe('SignalingClient', () => {
  it('sends join payload on connect and receives messages', async () => {
    const socket = new FakeWebSocket();
    const clientPromise = SignalingClient.connect({
      url: 'ws://example.invalid/ws/session',
      peerId: 'peer-123',
      createSocket: () => socket as unknown as WebSocket,
      supportedTransports: ['webrtc'],
    });

    socket.simulateOpen();
    const client = await clientPromise;

    expect(socket.sentMessages).toHaveLength(1);
    const joinPayload = JSON.parse(socket.sentMessages[0] as string);
    expect(joinPayload).toMatchObject({
      type: 'join',
      peer_id: 'peer-123',
      supported_transports: ['webrtc'],
    });

    const waitForSuccess = client.waitForMessage('join_success', 1000);
    socket.simulateMessage({
      type: 'join_success',
      session_id: 'session-42',
      peer_id: 'peer-123',
      peers: [],
      available_transports: ['webrtc'],
    });

    const message = await waitForSuccess;
    expect(message.session_id).toBe('session-42');
  });
});

class FakeWebSocket extends EventTarget {
  readyState = 0;
  binaryType: BinaryType = 'blob';
  sentMessages: Array<string | ArrayBufferLike> = [];

  send(data: string | ArrayBufferLike): void {
    this.sentMessages.push(data);
  }

  close(): void {
    const event = new Event('close');
    this.dispatchEvent(event);
  }

  simulateOpen(): void {
    this.readyState = 1;
    this.dispatchEvent(new Event('open'));
  }

  simulateMessage(message: ServerMessage): void {
    const event = new Event('message') as MessageEvent;
    Object.assign(event, { data: JSON.stringify(message) });
    this.dispatchEvent(event);
  }
}
