export const MAX_SAFE_U53 = Number.MAX_SAFE_INTEGER;

export function writeVarUint(value: number, out: number[]): void {
  if (!Number.isInteger(value) || value < 0 || value > MAX_SAFE_U53) {
    throw new RangeError(`invalid unsigned integer: ${value}`);
  }
  let current = value;
  while (current >= 0x80) {
    out.push((current & 0x7f) | 0x80);
    current = Math.floor(current / 0x80);
  }
  out.push(current);
}

export function readVarUint(buffer: Uint8Array, offset: { value: number }): number {
  let result = 0;
  let shift = 0;
  while (true) {
    if (offset.value >= buffer.length) {
      throw new RangeError('unexpected end of input while reading varint');
    }
    const byte = buffer[offset.value++];
    result += (byte & 0x7f) * 2 ** shift;
    if ((byte & 0x80) === 0) {
      break;
    }
    shift += 7;
    if (shift > 53) {
      throw new RangeError('varint overflow');
    }
  }
  if (result > MAX_SAFE_U53) {
    throw new RangeError('varint exceeds Number.MAX_SAFE_INTEGER');
  }
  return result;
}

export function readBytes(buffer: Uint8Array, length: number, offset: { value: number }): Uint8Array {
  const end = offset.value + length;
  if (end > buffer.length) {
    throw new RangeError('unexpected end of input while slicing bytes');
  }
  const view = buffer.subarray(offset.value, end);
  offset.value = end;
  return view;
}

export function writeBytes(bytes: Uint8Array, out: number[]): void {
  for (let index = 0; index < bytes.length; index += 1) {
    out.push(bytes[index]!);
  }
}
