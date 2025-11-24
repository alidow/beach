const FRAME_VERSION = 0xa1;
const FLAG_MAC_PRESENT = 0x1;
const DEFAULT_CHUNK_SIZE = 14 * 1024;
const DEFAULT_TIMEOUT_MS = 5_000;
const DEFAULT_MAX_INFLIGHT = 512;
const DEFAULT_MAX_BYTES = 8 * 1024 * 1024;
const RECENT_LIMIT = 256;
const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

export interface FramingConfig {
  chunkSize: number;
  timeoutMs: number;
  maxInflight: number;
  maxBytes: number;
}

export interface FramedMessage {
  namespace: string;
  kind: string;
  seq: number;
  payload: Uint8Array;
  totalLen: number;
}

type ParsedFrame = {
  namespace: string;
  kind: string;
  seq: number;
  totalLen: number;
  chunkIndex: number;
  chunkCount: number;
  crc32c: number;
  payload: Uint8Array;
  hasMac: boolean;
};

type Assembly = {
  createdAt: number;
  chunkCount: number;
  totalLen: number;
  crc32c: number;
  receivedBytes: number;
  chunks: Array<Uint8Array | null>;
};

const CRC32C_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i++) {
    let crc = i;
    for (let j = 0; j < 8; j++) {
      if ((crc & 1) !== 0) {
        crc = (crc >>> 1) ^ 0x82f63b78;
      } else {
        crc >>>= 1;
      }
    }
    table[i] = crc >>> 0;
  }
  return table;
})();

function crc32c(data: Uint8Array): number {
  let crc = 0xffffffff;
  for (let i = 0; i < data.length; i++) {
    const idx = (crc ^ data[i]) & 0xff;
    crc = CRC32C_TABLE[idx] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function readEnv(name: string): string | undefined {
  if (typeof import.meta !== 'undefined' && (import.meta as any).env && (import.meta as any).env[name] !== undefined) {
    return String((import.meta as any).env[name]);
  }
  if (typeof process !== 'undefined' && process.env && process.env[name] !== undefined) {
    return process.env[name];
  }
  return undefined;
}

function parseEnvInt(name: string, fallback: number, min?: number): number {
  const raw = readEnv(name);
  if (raw == null || raw.trim() === '') return fallback;
  const parsed = Number.parseInt(raw, 10);
  if (!Number.isFinite(parsed)) {
    return fallback;
  }
  if (min !== undefined && parsed < min) {
    return min;
  }
  return parsed;
}

export function runtimeFramingConfig(): FramingConfig {
  return {
    chunkSize: parseEnvInt('BEACH_FRAMED_CHUNK_SIZE', DEFAULT_CHUNK_SIZE, 512),
    timeoutMs: parseEnvInt('BEACH_FRAMED_TIMEOUT_MS', DEFAULT_TIMEOUT_MS, 1),
    maxInflight: parseEnvInt('BEACH_FRAMED_MAX_INFLIGHT', DEFAULT_MAX_INFLIGHT, 1),
    maxBytes: parseEnvInt('BEACH_FRAMED_MAX_BYTES', DEFAULT_MAX_BYTES, 1024),
  };
}

function encodeFrame(
  namespace: string,
  kind: string,
  seq: number,
  payload: Uint8Array,
  totalLen: number,
  chunkIndex: number,
  chunkCount: number,
  crc32: number,
): Uint8Array {
  if (namespace.length > 0xff || kind.length > 0xff) {
    throw new RangeError('namespace/kind too long for framing header');
  }
  const headerLen =
    2 + // version + flags
    0 + // mac key id (unused)
    1 +
    1 +
    namespace.length +
    kind.length +
    8 +
    4 +
    2 +
    2 +
    4;
  const buffer = new Uint8Array(headerLen + payload.length);
  let offset = 0;
  buffer[offset++] = FRAME_VERSION;
  buffer[offset++] = 0; // flags (no MAC)
  buffer[offset++] = namespace.length;
  buffer[offset++] = kind.length;
  buffer.set(textEncoder.encode(namespace), offset);
  offset += namespace.length;
  buffer.set(textEncoder.encode(kind), offset);
  offset += kind.length;
  const view = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);
  view.setBigUint64(offset, BigInt(seq), false);
  offset += 8;
  view.setUint32(offset, totalLen >>> 0, false);
  offset += 4;
  view.setUint16(offset, chunkIndex, false);
  offset += 2;
  view.setUint16(offset, chunkCount, false);
  offset += 2;
  view.setUint32(offset, crc32 >>> 0, false);
  offset += 4;
  buffer.set(payload, offset);
  return buffer;
}

function parseFrame(bytes: Uint8Array, config: FramingConfig): ParsedFrame {
  let offset = 0;
  if (bytes.length < 2) {
    throw new RangeError('frame too short');
  }
  const version = bytes[offset++];
  if (version !== FRAME_VERSION) {
    throw new RangeError(`unsupported frame version ${version}`);
  }
  const flags = bytes[offset++];
  const hasMac = (flags & FLAG_MAC_PRESENT) === FLAG_MAC_PRESENT;
  if (hasMac) {
    // MAC support not wired into the JS transport yet; fail fast to surface the mismatch.
    throw new RangeError('mac-protected frames are not supported in the browser transport');
  }
  if (offset + 2 > bytes.length) {
    throw new RangeError('frame missing namespace/kind lengths');
  }
  const namespaceLen = bytes[offset++];
  const kindLen = bytes[offset++];
  if (namespaceLen === 0 || kindLen === 0) {
    throw new RangeError('namespace/kind length missing');
  }
  if (offset + namespaceLen + kindLen + 8 + 4 + 2 + 2 + 4 > bytes.length) {
    throw new RangeError('frame header truncated');
  }
  const namespaceBytes = bytes.subarray(offset, offset + namespaceLen);
  offset += namespaceLen;
  const kindBytes = bytes.subarray(offset, offset + kindLen);
  offset += kindLen;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const seq = Number(view.getBigUint64(offset, false));
  offset += 8;
  const totalLen = view.getUint32(offset, false);
  offset += 4;
  const chunkIndex = view.getUint16(offset, false);
  offset += 2;
  const chunkCount = view.getUint16(offset, false);
  offset += 2;
  const crc = view.getUint32(offset, false);
  offset += 4;

  if (chunkCount === 0 || chunkIndex >= chunkCount) {
    throw new RangeError('invalid chunk index/count');
  }
  if (totalLen > config.maxBytes) {
    throw new RangeError(`framed payload exceeds limit (${totalLen})`);
  }

  const payload = bytes.subarray(offset);
  const namespace = textDecoder.decode(namespaceBytes);
  const kind = textDecoder.decode(kindBytes);
  return {
    namespace,
    kind,
    seq,
    totalLen,
    chunkIndex,
    chunkCount,
    crc32c: crc,
    payload,
    hasMac,
  };
}

function frameKey(frame: ParsedFrame): string {
  return `${frame.namespace}|${frame.kind}|${frame.seq}`;
}

export class FramedReassembler {
  private readonly assemblies = new Map<string, Assembly>();
  private readonly recent: Array<{ key: string; at: number }> = [];

  ingest(frame: ParsedFrame, config: FramingConfig, nowMs: number): FramedMessage | null {
    this.gc(nowMs, config);

    const key = frameKey(frame);
    if (this.recent.some((entry) => entry.key === key)) {
      return null;
    }

    if (frame.chunkCount === 1 && frame.chunkIndex === 0) {
      if (frame.totalLen !== frame.payload.length) {
        throw new RangeError('payload length mismatch');
      }
      const crc = crc32c(frame.payload);
      if (crc !== frame.crc32c) {
        throw new RangeError('crc mismatch');
      }
      this.recordRecent(key, nowMs);
      return {
        namespace: frame.namespace,
        kind: frame.kind,
        seq: frame.seq,
        payload: frame.payload,
        totalLen: frame.totalLen,
      };
    }

    const existing = this.assemblies.get(key);
    if (existing && (existing.chunkCount !== frame.chunkCount || existing.totalLen !== frame.totalLen)) {
      this.assemblies.delete(key);
    }

    let assembly = this.assemblies.get(key);
    if (!assembly) {
      if (this.assemblies.size >= config.maxInflight) {
        const oldest = [...this.assemblies.entries()].sort((a, b) => a[1].createdAt - b[1].createdAt)[0];
        if (oldest) {
          this.assemblies.delete(oldest[0]);
        }
      }
      assembly = {
        createdAt: nowMs,
        chunkCount: frame.chunkCount,
        totalLen: frame.totalLen,
        crc32c: frame.crc32c,
        receivedBytes: 0,
        chunks: new Array(frame.chunkCount).fill(null),
      };
      this.assemblies.set(key, assembly);
    }

    if (!assembly.chunks[frame.chunkIndex]) {
      assembly.chunks[frame.chunkIndex] = frame.payload;
      assembly.receivedBytes += frame.payload.length;
      if (assembly.receivedBytes > config.maxBytes || assembly.receivedBytes > assembly.totalLen) {
        this.assemblies.delete(key);
        throw new RangeError('framed payload exceeds limits during assembly');
      }
    }

    if (assembly.chunks.every((chunk) => chunk != null)) {
      const combined = new Uint8Array(assembly.totalLen);
      let offset = 0;
      for (const chunk of assembly.chunks) {
        if (!chunk) {
          this.assemblies.delete(key);
          throw new RangeError('missing chunk during assembly');
        }
        combined.set(chunk, offset);
        offset += chunk.length;
      }
      this.assemblies.delete(key);
      const crc = crc32c(combined);
      if (crc !== assembly.crc32c) {
        throw new RangeError('crc mismatch after assembly');
      }
      this.recordRecent(key, nowMs);
      return {
        namespace: frame.namespace,
        kind: frame.kind,
        seq: frame.seq,
        payload: combined,
        totalLen: assembly.totalLen,
      };
    }

    return null;
  }

  private gc(nowMs: number, config: FramingConfig): void {
    for (const [key, value] of this.assemblies.entries()) {
      if (nowMs - value.createdAt > config.timeoutMs) {
        this.assemblies.delete(key);
      }
    }
    while (this.recent.length > RECENT_LIMIT) {
      this.recent.shift();
    }
  }

  private recordRecent(key: string, nowMs: number): void {
    this.recent.push({ key, at: nowMs });
    if (this.recent.length > RECENT_LIMIT) {
      this.recent.shift();
    }
  }
}

export function encodeFramedMessage(
  namespace: string,
  kind: string,
  seq: number,
  payload: Uint8Array,
  config: FramingConfig,
): Uint8Array[] {
  if (payload.length > config.maxBytes) {
    throw new RangeError(`framed payload exceeds limit (${payload.length})`);
  }
  const chunkSize = Math.max(1, config.chunkSize);
  const chunkCount = payload.length === 0 ? 1 : Math.ceil(payload.length / chunkSize);
  if (chunkCount > 0xffff) {
    throw new RangeError('framed payload requires too many chunks');
  }
  const crc = crc32c(payload);
  if (payload.length === 0) {
    return [encodeFrame(namespace, kind, seq, new Uint8Array(0), 0, 0, 1, crc)];
  }
  const frames: Uint8Array[] = [];
  for (let chunkIndex = 0; chunkIndex < chunkCount; chunkIndex++) {
    const start = chunkIndex * chunkSize;
    const end = Math.min(start + chunkSize, payload.length);
    const slice = payload.subarray(start, end);
    frames.push(encodeFrame(namespace, kind, seq, slice, payload.length, chunkIndex, chunkCount, crc));
  }
  return frames;
}

export function decodeFramedMessage(
  bytes: Uint8Array,
  reassembler: FramedReassembler,
  config: FramingConfig,
  nowMs: number,
):
  | { kind: 'incomplete' }
  | {
      kind: 'complete';
      frame: FramedMessage;
    } {
  const parsed = parseFrame(bytes, config);
  const assembled = reassembler.ingest(parsed, config, nowMs);
  if (!assembled) {
    return { kind: 'incomplete' };
  }
  return { kind: 'complete', frame: assembled };
}
