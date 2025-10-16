export type TransportPayload =
  | { kind: 'text'; text: string }
  | { kind: 'binary'; data: Uint8Array };

export interface TransportMessage {
  sequence: number;
  payload: TransportPayload;
}

const HEADER_SIZE = 1 + 8 + 4; // type + sequence + length

function ensureSequence(value: number): number {
  if (!Number.isInteger(value) || value < 0 || value > Number.MAX_SAFE_INTEGER) {
    throw new RangeError(`invalid transport sequence: ${value}`);
  }
  return value;
}

export function encodeTransportMessage(message: TransportMessage): Uint8Array {
  const sequence = ensureSequence(message.sequence);
  const payloadBytes =
    message.payload.kind === 'binary'
      ? message.payload.data
      : new TextEncoder().encode(message.payload.text);
  const total = HEADER_SIZE + payloadBytes.length;
  const buffer = new Uint8Array(total);
  let offset = 0;

  buffer[offset++] = message.payload.kind === 'text' ? 0 : 1;

  const seqView = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);
  seqView.setBigUint64(offset, BigInt(sequence), false);
  offset += 8;
  seqView.setUint32(offset, payloadBytes.length, false);
  offset += 4;

  buffer.set(payloadBytes, offset);
  return buffer;
}

export function decodeTransportMessage(input: ArrayBuffer | Uint8Array): TransportMessage {
  const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
  if (bytes.length < HEADER_SIZE) {
    throw new RangeError('transport frame too short');
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let offset = 0;
  const kindByte = bytes[offset++];
  const sequence = Number(view.getBigUint64(offset, false));
  ensureSequence(sequence);
  offset += 8;
  const length = view.getUint32(offset, false);
  offset += 4;
  if (length > bytes.length - offset) {
    throw new RangeError('transport payload truncated');
  }
  const payloadSlice = bytes.subarray(offset, offset + length);
  if (kindByte === 0) {
    const text = new TextDecoder().decode(payloadSlice);
    return { sequence, payload: { kind: 'text', text } };
  }
  if (kindByte === 1) {
    return { sequence, payload: { kind: 'binary', data: payloadSlice.slice() } };
  }
  throw new RangeError(`unknown transport payload kind: ${kindByte}`);
}
