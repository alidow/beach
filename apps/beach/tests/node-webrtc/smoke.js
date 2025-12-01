import WebSocket from 'ws';
import { RTCPeerConnection } from 'werift';
import { randomUUID, createHmac } from 'node:crypto';
import CRC32 from 'crc-32';

const FRAME_VERSION = 0xa1;
const FLAG_MAC_PRESENT = 0x1;
const MAC_TAG_LEN = 32;
const DEFAULT_ROAD_URL = process.env.ROAD_URL || 'http://localhost:4132';
const HOST_SESSION_ID = process.env.HOST_SESSION_ID;
const HOST_PASSCODE = process.env.HOST_PASSCODE;
const CONTROLLER_PASSCODE = process.env.CONTROLLER_PASSCODE || HOST_PASSCODE;
const TRANSPORT_NAME = 'webrtc';

function hmacTag(namespace, kind, seq, payload, keyId, key) {
  const hmac = createHmac('sha256', key);
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

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function requireEnv(name, value) {
  if (!value || !value.trim()) {
    throw new Error(`Missing required env ${name}`);
  }
  return value.trim();
}

function toWsUrl(baseUrl, sessionId) {
  const url = new URL(baseUrl);
  if (url.protocol === 'http:') url.protocol = 'ws:';
  if (url.protocol === 'https:') url.protocol = 'wss:';
  return new URL(`/ws/${sessionId}`, url).toString();
}

function toHttpBase(baseUrl) {
  const url = new URL(baseUrl);
  if (url.protocol === 'ws:') url.protocol = 'http:';
  if (url.protocol === 'wss:') url.protocol = 'https:';
  return url;
}

async function fetchHostPeerId(httpBase, sessionId) {
  try {
    const url = new URL(`/debug/sessions/${sessionId}/peers`, httpBase);
    const resp = await fetch(url);
    if (!resp.ok) {
      return null;
    }
    const data = await resp.json();
    const peers = Array.isArray(data) ? data : Array.isArray(data?.peers) ? data.peers : [];
    const host = peers.find((peer) => peer.role === 'server');
    return host?.id ?? null;
  } catch (error) {
    console.warn(`[debug-endpoint] failed to fetch peers: ${String(error)}`);
    return null;
  }
}

function createSignaling(wsUrl, { passcode, label }) {
  const ws = new WebSocket(wsUrl);
  const handlers = new Set();
  const waiters = new Set();
  let closedError = null;

  const failWaiters = (err) => {
    if (closedError) return;
    closedError = err instanceof Error ? err : new Error(String(err));
    for (const waiter of Array.from(waiters)) {
      clearTimeout(waiter.timer);
      waiters.delete(waiter);
      waiter.reject(closedError);
    }
  };

  const dispatch = (message) => {
    for (const waiter of Array.from(waiters)) {
      if (!waiter.predicate(message)) continue;
      clearTimeout(waiter.timer);
      waiters.delete(waiter);
      waiter.resolve(message);
    }
    for (const handler of handlers) {
      try {
        handler(message);
      } catch (error) {
        console.warn('[signaling] handler error', error);
      }
    }
  };

  ws.on('message', (raw) => {
    try {
      const parsed = JSON.parse(typeof raw === 'string' ? raw : raw.toString());
      dispatch(parsed);
    } catch (error) {
      console.warn('[signaling] failed to parse message', error);
    }
  });
  ws.on('close', (code, reason) => {
    const suffix = reason ? ` ${reason.toString()}` : '';
    failWaiters(new Error(`signaling closed (${code}${suffix})`));
  });
  ws.on('error', (err) => failWaiters(err));

  const waitFor = (predicate, timeoutMs, label) =>
    new Promise((resolve, reject) => {
      if (closedError) {
        reject(closedError);
        return;
      }
      let waiter = null;
      const timer = setTimeout(() => {
        if (waiter) waiters.delete(waiter);
        reject(new Error(`Timed out waiting for ${label}`));
      }, timeoutMs);
      waiter = { predicate, resolve, reject, timer };
      waiters.add(waiter);
    });

  const onMessage = (fn) => {
    handlers.add(fn);
    return () => handlers.delete(fn);
  };

  const ready = new Promise((resolve, reject) => {
    ws.once('open', resolve);
    ws.once('error', reject);
  });

  const join = async () => {
    await ready;
    const requestedPeer = randomUUID();
    ws.send(
      JSON.stringify({
        type: 'join',
        peer_id: requestedPeer,
        passphrase: passcode ?? null,
        supported_transports: [TRANSPORT_NAME],
        preferred_transport: TRANSPORT_NAME,
        label: label ?? null,
      }),
    );
    const joinMsg = await waitFor(
      (msg) => msg.type === 'join_success' || msg.type === 'join_error',
      10_000,
      'join response',
    );
    if (joinMsg.type === 'join_error') {
      throw new Error(`join failed: ${joinMsg.reason}`);
    }
    return joinMsg;
  };

  return { ws, waitFor, onMessage, join, close: () => ws.close() };
}

function parseWebRtcSignal(message, fromPeer) {
  if (!message || message.type !== 'signal' || message.from_peer !== fromPeer) {
    return null;
  }
  const envelope = message.signal;
  if (!envelope || envelope.transport !== TRANSPORT_NAME) {
    return null;
  }
  const signal = envelope.signal;
  if (!signal || typeof signal !== 'object') {
    return null;
  }
  if (signal.signal_type === 'offer' || signal.signal_type === 'answer') {
    if (typeof signal.handshake_id !== 'string') return null;
    return {
      kind: signal.signal_type,
      handshakeId: signal.handshake_id,
      sdp: typeof signal.sdp === 'string' ? signal.sdp : '',
    };
  }
  if (signal.signal_type === 'ice_candidate') {
    if (typeof signal.handshake_id !== 'string' || typeof signal.candidate !== 'string') {
      return null;
    }
    return {
      kind: 'ice_candidate',
      handshakeId: signal.handshake_id,
      candidate: signal.candidate,
      sdpMid: typeof signal.sdp_mid === 'string' ? signal.sdp_mid : undefined,
      sdpMLineIndex:
        typeof signal.sdp_mline_index === 'number' ? signal.sdp_mline_index : undefined,
    };
  }
  return null;
}

function isWebRtcTransport(value) {
  if (!value) return false;
  if (typeof value === 'string') return value === TRANSPORT_NAME;
  if (typeof value === 'object' && 'custom' in value) return false;
  return value === TRANSPORT_NAME;
}

async function negotiateTransport(signaling, hostPeerId) {
  signaling.ws.send(
    JSON.stringify({
      type: 'negotiate_transport',
      to_peer: hostPeerId,
      proposed_transport: TRANSPORT_NAME,
    }),
  );

  const accept = signaling.waitFor(
    (msg) =>
      msg.type === 'transport_accepted' &&
      msg.from_peer === hostPeerId &&
      isWebRtcTransport(msg.transport),
    10_000,
    'transport acceptance',
  );

  const proposalHandler = signaling.onMessage((msg) => {
    if (
      msg.type === 'transport_proposal' &&
      msg.from_peer === hostPeerId &&
      isWebRtcTransport(msg.proposed_transport)
    ) {
      signaling.ws.send(
        JSON.stringify({
          type: 'accept_transport',
          to_peer: hostPeerId,
          transport: msg.proposed_transport,
        }),
      );
    }
  });

  try {
    await accept;
  } finally {
    proposalHandler();
  }
}

function waitForChannelOpen(channel, timeoutMs) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('data channel open timeout')), timeoutMs);
    channel.onopen = () => {
      clearTimeout(timer);
      resolve();
    };
    channel.onerror = (err) => {
      clearTimeout(timer);
      reject(err);
    };
  });
}

async function establishWebRtc(signaling, hostPeerId, localPeerId) {
  const offerer = localPeerId.localeCompare(hostPeerId) < 0;
  const pc = new RTCPeerConnection({
    iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
  });

  let handshakeId = null;
  const pendingRemoteCandidates = [];
  let remoteDescriptionSet = false;
  const sendSignal = (signal) => {
    signaling.ws.send(
      JSON.stringify({
        type: 'signal',
        to_peer: hostPeerId,
        signal: {
          transport: TRANSPORT_NAME,
          signal,
        },
      }),
    );
  };

  const handleRemoteCandidate = async (candidate) => {
    if (!remoteDescriptionSet) {
      pendingRemoteCandidates.push(candidate);
      return;
    }
    try {
      await pc.addIceCandidate(candidate);
    } catch (error) {
      console.warn('[webrtc] failed to apply remote candidate', error);
    }
  };

  const flushPendingRemote = async () => {
    while (pendingRemoteCandidates.length > 0) {
      const cand = pendingRemoteCandidates.shift();
      if (cand) {
        await handleRemoteCandidate(cand);
      }
    }
  };

  const pendingLocalCandidates = [];
  pc.onicecandidate = (event) => {
    if (!event.candidate) return;
    const candidate = event.candidate.toJSON();
    const payload = {
      signal_type: 'ice_candidate',
      handshake_id: handshakeId,
      candidate: candidate.candidate ?? '',
      sdp_mid: candidate.sdpMid ?? undefined,
      sdp_mline_index: candidate.sdpMLineIndex ?? undefined,
    };
    if (!handshakeId) {
      pendingLocalCandidates.push(payload);
      return;
    }
    sendSignal(payload);
  };

  let dataChannelPromise;
  let primaryChannel;
  if (offerer) {
    handshakeId = randomUUID();
    primaryChannel = pc.createDataChannel('beach', { ordered: true });
    dataChannelPromise = waitForChannelOpen(primaryChannel, 20_000);
  } else {
    dataChannelPromise = new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('data channel not announced')), 20_000);
      pc.ondatachannel = (event) => {
        if (event.channel.label !== 'beach') {
          return;
        }
        primaryChannel = event.channel;
        clearTimeout(timer);
        resolve();
      };
    });
  }

  const iceHandler = signaling.onMessage((msg) => {
    const parsed = parseWebRtcSignal(msg, hostPeerId);
    if (!parsed || parsed.kind !== 'ice_candidate') return;
    if (!handshakeId || parsed.handshakeId !== handshakeId) return;
    const cand = {
      candidate: parsed.candidate,
      sdpMid: parsed.sdpMid,
      sdpMLineIndex: parsed.sdpMLineIndex,
    };
    void handleRemoteCandidate(cand);
  });

  try {
    if (offerer) {
      const offer = await pc.createOffer();
      await pc.setLocalDescription(offer);
      sendSignal({
        signal_type: 'offer',
        handshake_id: handshakeId,
        sdp: offer.sdp ?? '',
      });
      const { signal: answerSignal } = await signaling.waitFor(
        (msg) => {
          const parsed = parseWebRtcSignal(msg, hostPeerId);
          if (!parsed || parsed.kind !== 'answer') return false;
          return parsed.handshakeId === handshakeId;
        },
        15_000,
        'remote answer',
      );
      const parsedAnswer = parseWebRtcSignal(
        { type: 'signal', from_peer: hostPeerId, signal: answerSignal },
        hostPeerId,
      );
      await pc.setRemoteDescription({ type: 'answer', sdp: parsedAnswer?.sdp ?? '' });
      remoteDescriptionSet = true;
      await flushPendingRemote();
    } else {
      const offerMsg = await signaling.waitFor(
        (msg) => {
          const parsed = parseWebRtcSignal(msg, hostPeerId);
          return parsed?.kind === 'offer';
        },
        20_000,
        'remote offer',
      );
      const offerSignal = parseWebRtcSignal(offerMsg, hostPeerId);
      handshakeId = offerSignal?.handshakeId ?? randomUUID();
      await pc.setRemoteDescription({ type: 'offer', sdp: offerSignal?.sdp ?? '' });
      remoteDescriptionSet = true;
      await flushPendingRemote();

      const answer = await pc.createAnswer();
      await pc.setLocalDescription(answer);
      sendSignal({
        signal_type: 'answer',
        handshake_id: handshakeId,
        sdp: answer.sdp ?? '',
      });
      while (pendingLocalCandidates.length > 0) {
        const cand = pendingLocalCandidates.shift();
        if (cand) {
          cand.handshake_id = handshakeId;
          sendSignal(cand);
        }
      }
    }

    await dataChannelPromise;
    return { pc, channel: primaryChannel };
  } finally {
    iceHandler();
  }
}

async function exchangePing(channel, label) {
  channel.binaryType = 'arraybuffer';
  const payload = Buffer.from(
    JSON.stringify({ action: 'ping', label, ts: Date.now(), nonce: randomUUID() }),
  );
  const frame = encodeFrame('controller', 'input', 1, payload);
  channel.send(frame);

  const ack = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('ack timeout')), 8_000);
    channel.onmessage = (event) => {
      clearTimeout(timer);
      try {
        resolve(Buffer.from(event.data));
      } catch (error) {
        reject(error);
      }
    };
    channel.onerror = (err) => {
      clearTimeout(timer);
      reject(err);
    };
  });

  const decoded = decodeFrame(ack);
  if (decoded.namespace !== 'controller' || decoded.kind !== 'ack') {
    throw new Error('unexpected ack frame');
  }
}

async function runLeg({ label, passcode, roadUrl, sessionId }) {
  console.log(`\n[${label}] starting WebRTC smoke`);
  const wsUrl = toWsUrl(roadUrl, sessionId);
  const signaling = createSignaling(wsUrl, { passcode, label });
  const join = await signaling.join();
  console.log(`[${label}] joined session as ${join.peer_id}`);

  let hostPeerId =
    join.peers.find((peer) => peer.role === 'server')?.id ??
    (await fetchHostPeerId(toHttpBase(roadUrl), sessionId));
  if (!hostPeerId) {
    const serverPeer = await signaling.waitFor(
      (msg) => msg.type === 'peer_joined' && msg.peer.role === 'server',
      15_000,
      'host peer',
    );
    hostPeerId = serverPeer.peer.id;
  }
  console.log(`[${label}] host peer ${hostPeerId}`);

  await negotiateTransport(signaling, hostPeerId);
  console.log(`[${label}] transport negotiation accepted`);

  const { pc, channel } = await establishWebRtc(signaling, hostPeerId, join.peer_id);
  await exchangePing(channel, label);
  console.log(`[${label}] ping/ack over data channel ok`);
  channel.close();
  await sleep(200);
  pc.close();
  signaling.close();
}

async function main() {
  const sessionId = requireEnv('HOST_SESSION_ID', HOST_SESSION_ID);
  const hostPass = requireEnv('HOST_PASSCODE', HOST_PASSCODE);
  const controllerPass = CONTROLLER_PASSCODE || hostPass;

  try {
    await runLeg({
      label: 'private-beach-dashboard',
      passcode: hostPass,
      roadUrl: DEFAULT_ROAD_URL,
      sessionId,
    });
    await runLeg({
      label: 'beach-manager',
      passcode: controllerPass,
      roadUrl: DEFAULT_ROAD_URL,
      sessionId,
    });
    console.log('\n✅ WebRTC smoke completed');
    process.exit(0);
  } catch (error) {
    console.error('\n❌ WebRTC smoke failed', error);
    process.exit(1);
  }
}

main();
