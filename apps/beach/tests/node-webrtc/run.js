import assert from 'node:assert';
import CRC32 from 'crc-32';
import { RTCPeerConnection } from 'werift';
import { setTimeout as wait } from 'node:timers/promises';
import crypto from 'node:crypto';

const FRAME_VERSION = 0xa1;
const FLAG_MAC_PRESENT = 0x1;
const MAC_TAG_LEN = 32;

function hmacTag(namespace, kind, seq, payload, keyId, key) {
  const hmac = crypto.createHmac('sha256', key);
  hmac.update(Buffer.from([FRAME_VERSION]));
  hmac.update(Buffer.from([namespace.length]));
  hmac.update(Buffer.from(namespace));
  hmac.update(Buffer.from([kind.length]));
  hmac.update(Buffer.from(kind));
  const seqBuf = Buffer.alloc(8);
  seqBuf.writeBigUInt64BE(BigInt(seq));
  hmac.update(seqBuf);
  const totalBuf = Buffer.alloc(4);
  totalBuf.writeUInt32BE(payload.length, 0);
  hmac.update(totalBuf);
  hmac.update(payload);
  return { keyId, tag: hmac.digest().subarray(0, MAC_TAG_LEN) };
}

function encodeFrame(namespace, kind, seq, payload, macCfg) {
  const ns = Buffer.from(namespace);
  const kd = Buffer.from(kind);
  const crc = CRC32.buf(payload) >>> 0;
  const seqBuf = Buffer.alloc(8);
  seqBuf.writeBigUInt64BE(BigInt(seq));
  const totalBuf = Buffer.alloc(4);
  totalBuf.writeUInt32BE(payload.length, 0);
  const chunkIndex = Buffer.alloc(2);
  chunkIndex.writeUInt16BE(0);
  const chunkCount = Buffer.alloc(2);
  chunkCount.writeUInt16BE(1);
  let macHeader = Buffer.alloc(0);
  let macTag = Buffer.alloc(0);
  let flags = 0;
  if (macCfg && macCfg.key && macCfg.keyId !== undefined) {
    const mac = hmacTag(namespace, kind, seq, payload, macCfg.keyId, macCfg.key);
    flags |= FLAG_MAC_PRESENT;
    macHeader = Buffer.from([mac.keyId]);
    macTag = mac.tag;
  }
  const header = Buffer.concat([
    Buffer.from([FRAME_VERSION, flags]),
    macHeader,
    Buffer.from([ns.length, kd.length]),
    ns,
    kd,
    seqBuf,
    totalBuf,
    chunkIndex,
    chunkCount,
    Buffer.from([(crc >>> 24) & 0xff, (crc >>> 16) & 0xff, (crc >>> 8) & 0xff, crc & 0xff]),
  ]);
  return Buffer.concat([header, payload, macTag]);
}

function decodeFrame(buf, macCfg) {
  let offset = 0;
  const version = buf.readUInt8(offset++);
  if (version !== FRAME_VERSION) throw new Error('bad version');
  const flags = buf.readUInt8(offset++);
  const macPresent = (flags & FLAG_MAC_PRESENT) !== 0;
  let keyId;
  if (macPresent) keyId = buf.readUInt8(offset++);
  const nsLen = buf.readUInt8(offset++);
  const kindLen = buf.readUInt8(offset++);
  const namespace = buf.slice(offset, offset + nsLen).toString('utf8');
  offset += nsLen;
  const kind = buf.slice(offset, offset + kindLen).toString('utf8');
  offset += kindLen;
  const seq = Number(buf.readBigUInt64BE(offset));
  offset += 8;
  const totalLen = buf.readUInt32BE(offset);
  offset += 4;
  const chunkIndex = buf.readUInt16BE(offset);
  offset += 2;
  const chunkCount = buf.readUInt16BE(offset);
  offset += 2;
  const crc = buf.readUInt32BE(offset);
  offset += 4;
  const payload = buf.slice(offset, buf.length - (macPresent ? MAC_TAG_LEN : 0));
  offset = buf.length - (macPresent ? MAC_TAG_LEN : 0);
  const macTag = macPresent ? buf.slice(offset) : null;
  const crcCheck = CRC32.buf(payload) >>> 0;
  if (crcCheck !== crc) throw new Error('crc mismatch');
  if (macPresent && macCfg) {
    const mac = hmacTag(namespace, kind, seq, payload, keyId, macCfg.key);
    if (!mac.tag.equals(macTag)) throw new Error('mac mismatch');
  }
  return { namespace, kind, seq, totalLen, chunkIndex, chunkCount, payload };
}

async function createPair() {
  const offerer = new RTCPeerConnection({ iceServers: [] });
  const answerer = new RTCPeerConnection({ iceServers: [] });

  const channel = offerer.createDataChannel('pb-controller', { ordered: true });
  let serverChannel;
  answerer.ondatachannel = (ev) => {
    serverChannel = ev.channel;
  };

  const offer = await offerer.createOffer();
  await offerer.setLocalDescription(offer);
  await answerer.setRemoteDescription(offerer.localDescription);
  const answer = await answerer.createAnswer();
  await answerer.setLocalDescription(answer);
  await offerer.setRemoteDescription(answerer.localDescription);

  await wait(50);

  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('channel open timeout')), 5000);
    channel.onopen = () => {
      clearTimeout(timer);
      resolve();
    };
    channel.onerror = (err) => {
      clearTimeout(timer);
      reject(err);
    };
  });

  return { offerer, answerer, channel, serverChannel };
}

async function main() {
  const macKey = Buffer.from('00112233445566778899aabbccddeeff', 'hex');
  const macCfg = { keyId: 1, key: macKey };

  const { channel, serverChannel, offerer, answerer } = await createPair();
  assert.ok(serverChannel, 'server datachannel established');

  const messages = [];
  serverChannel.onmessage = (ev) => messages.push(Buffer.from(ev.data));

  const payload = Buffer.from(JSON.stringify({ action: 'ping', id: 1 }));
  const frame = encodeFrame('controller', 'input', 1, payload, macCfg);
  channel.send(frame);

  await wait(200);
  assert.equal(messages.length, 1, 'server should receive one framed message');
  const decoded = decodeFrame(messages[0], macCfg);
  assert.equal(decoded.namespace, 'controller');
  assert.equal(decoded.kind, 'input');
  assert.equal(decoded.seq, 1);
  assert.equal(decoded.totalLen, payload.length);
  assert.deepEqual(decoded.payload, payload);

  const ackPayload = Buffer.from(JSON.stringify({ ack: 1 }));
  const ackFrame = encodeFrame('controller', 'ack', 2, ackPayload, macCfg);
  serverChannel.send(ackFrame);

  const ackRecv = await new Promise((resolve, reject) => {
    channel.onmessage = (ev) => resolve(Buffer.from(ev.data));
    channel.onerror = reject;
  });
  const decodedAck = decodeFrame(ackRecv, macCfg);
  assert.equal(decodedAck.namespace, 'controller');
  assert.equal(decodedAck.kind, 'ack');
  assert.equal(decodedAck.seq, 2);
  assert.deepEqual(decodedAck.payload, ackPayload);

  offerer.close();
  answerer.close();
}

main()
  .then(() => {
    console.log('node-werift framed round-trip ok');
  })
  .catch((err) => {
    console.error(err);
    process.exit(1);
  });
