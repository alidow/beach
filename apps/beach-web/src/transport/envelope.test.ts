import { describe, expect, it } from 'vitest';
import { decodeTransportMessage, encodeTransportMessage } from './envelope';

describe('transport envelope', () => {
  it('encodes and decodes text payloads', () => {
    const encoded = encodeTransportMessage({
      sequence: 12,
      payload: { kind: 'text', text: 'hello world' },
    });
    const decoded = decodeTransportMessage(encoded);
    expect(decoded).toEqual({ sequence: 12, payload: { kind: 'text', text: 'hello world' } });
  });

  it('encodes and decodes binary payloads', () => {
    const payload = Uint8Array.from([0xde, 0xad, 0xbe, 0xef]);
    const encoded = encodeTransportMessage({ sequence: 99, payload: { kind: 'binary', data: payload } });
    const decoded = decodeTransportMessage(encoded);
    expect(decoded.sequence).toBe(99);
    expect(decoded.payload.kind).toBe('binary');
    if (decoded.payload.kind === 'binary') {
      expect(decoded.payload.data).toEqual(payload);
    }
  });

  it('rejects frames with unknown payload type', () => {
    const frame = new Uint8Array([3, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0]);
    expect(() => decodeTransportMessage(frame)).toThrow(/unknown transport payload kind/);
  });
});
