#!/usr/bin/env node
import { WebSocket as NodeWebSocket } from 'ws';
import {
  RTCPeerConnection as WeriftPeerConnection,
  RTCSessionDescription as WeriftSessionDescription,
  RTCIceCandidate as WeriftIceCandidate,
  MediaStream as WeriftMediaStream,
} from 'werift';

import { connectBrowserTransport } from '../src/terminal/connect';

// Minimal CustomEvent polyfill for Node environments that lack it.
if (typeof globalThis.CustomEvent === 'undefined') {
  class NodeCustomEvent<T = unknown> extends Event {
    readonly detail: T;
    constructor(type: string, init?: { detail?: T }) {
      super(type);
      this.detail = init?.detail as T;
    }
  }
  // @ts-expect-error - assign polyfill
  globalThis.CustomEvent = NodeCustomEvent;
}

// Provide WebRTC globals using the wrtc package.
const anyGlobal = globalThis as any;
if (typeof anyGlobal.RTCPeerConnection === 'undefined') {
  anyGlobal.RTCPeerConnection = WeriftPeerConnection;
}
if (typeof anyGlobal.RTCSessionDescription === 'undefined') {
  anyGlobal.RTCSessionDescription = WeriftSessionDescription;
}
if (typeof anyGlobal.RTCIceCandidate === 'undefined') {
  anyGlobal.RTCIceCandidate = WeriftIceCandidate;
}
if (typeof anyGlobal.MediaStream === 'undefined') {
  anyGlobal.MediaStream = WeriftMediaStream;
}
if (typeof anyGlobal.navigator === 'undefined') {
  anyGlobal.navigator = { userAgent: 'node.js' };
}
if (typeof anyGlobal.crypto === 'undefined') {
  const { webcrypto } = await import('node:crypto');
  anyGlobal.crypto = webcrypto;
}

interface CliConfig {
  sessionId: string;
  baseUrl: string;
  passcode?: string;
  durationMs: number;
}

function parseArgs(argv: string[]): CliConfig {
  const [, , sessionId, baseUrl, passcode] = argv;
  if (!sessionId || !baseUrl) {
    console.error('Usage: pnpm debug:webrtc <session-id> <base-url> [passcode]');
    process.exit(1);
  }
  const durationRaw = process.env.BEACH_DEBUG_DURATION_MS;
  const durationMs = durationRaw ? Number(durationRaw) : 20_000;
  return {
    sessionId,
    baseUrl,
    passcode,
    durationMs: Number.isFinite(durationMs) && durationMs > 0 ? durationMs : 20_000,
  };
}

async function run(): Promise<void> {
  const config = parseArgs(process.argv);
  console.log('[webrtc-debug] starting connection test');
  console.log(`  session : ${config.sessionId}`);
  console.log(`  server  : ${config.baseUrl}`);
  if (config.passcode) {
    console.log('  passcode: provided');
  }

  const connection = await connectBrowserTransport({
    sessionId: config.sessionId,
    baseUrl: config.baseUrl,
    passcode: config.passcode,
    logger: (message) => console.log(`[webrtc-debug] ${message}`),
    createSocket: (url) => new NodeWebSocket(url),
  });

  connection.transport.addEventListener('frame', (event) => {
    const frame = (event as CustomEvent).detail;
    console.log(`[webrtc-debug] host frame received: ${frame.type}`);
  });
  connection.transport.addEventListener('error', (event) => {
    const err = (event as any).error;
    console.error('[webrtc-debug] transport error', err);
  });
  connection.transport.addEventListener('close', () => {
    console.log('[webrtc-debug] transport closed');
  });

  console.log('[webrtc-debug] connected; waiting for frames');
  const shutdown = () => {
    console.log('[webrtc-debug] closing connection');
    connection.close();
    process.exit(0);
  };

  const timeout = setTimeout(shutdown, config.durationMs);
  process.on('SIGINT', () => {
    clearTimeout(timeout);
    shutdown();
  });
}

run().catch((error) => {
  console.error('[webrtc-debug] failed', error);
  process.exit(1);
});
