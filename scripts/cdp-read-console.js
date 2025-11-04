#!/usr/bin/env node
/*
 * Connects to a running Chrome instance over the DevTools Protocol and prints console messages
 * for the specified page. Requires: npm i -D chrome-remote-interface
 *
 * Usage:
 *   node scripts/cdp-read-console.js --port 9222 --match "http://localhost:3001/beaches/.."
 */

const http = require('http');
const CDP = require('chrome-remote-interface');

function parseArgs() {
  const args = process.argv.slice(2);
  const out = { port: 9222, host: '127.0.0.1', match: '', duration: 10_000, reload: false };
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if ((a === '--port' || a === '-p') && args[i + 1]) { out.port = Number(args[++i]); }
    else if ((a === '--host' || a === '-t') && args[i + 1]) { out.host = args[++i]; }
    else if ((a === '--match' || a === '-m') && args[i + 1]) { out.match = args[++i]; }
    else if ((a === '--duration' || a === '-d') && args[i + 1]) {
      out.duration = Math.max(1, Math.floor(Number(args[++i]) * 1000));
    } else if (a === '--reload' || a === '-r') {
      out.reload = true;
    }
  }
  if (!out.match) {
    console.error('Error: --match <url-substring> is required');
    process.exit(2);
  }
  return out;
}

async function listTargets(host, port) {
  return new Promise((resolve, reject) => {
    const req = http.request({ host, port, path: '/json/list', method: 'GET' }, (res) => {
      let data = '';
      res.on('data', (c) => (data += c));
      res.on('end', () => {
        try { resolve(JSON.parse(data)); } catch (e) { reject(e); }
      });
    });
    req.on('error', reject);
    req.end();
  });
}

function formatArgs(args) {
  return args.map((a) => {
    try {
      if (a.type === 'string') return a.value;
      if (a.value !== undefined) return JSON.stringify(a.value);
      if (a.description) return a.description;
      return '[unserializable]';
    } catch { return '[unserializable]'; }
  }).join(' ');
}

(async () => {
  const { host, port, match, duration, reload } = parseArgs();
  const targets = await listTargets(host, port);
  const target = targets.find((t) => (t.type === 'page' || t.type === 'iframe') && t.url && t.url.includes(match));
  if (!target) {
    console.error(`No matching page found for match="${match}" on ${host}:${port}`);
    process.exit(1);
  }

  console.log(`Connecting to page: ${target.title || target.url}`);
  console.log(`Target ID: ${target.id}`);
  console.log(`ws: ${target.webSocketDebuggerUrl}`);

  // Connect directly using the target WebSocket URL for reliability
  const client = await CDP({ target: target.webSocketDebuggerUrl });
  const { Runtime, Console, Log, Page } = client;

  await Promise.all([Runtime.enable(), Console.enable(), Log.enable(), Page.enable()]);

  if (reload) {
    try {
      console.log('Reloading page…');
      await Page.reload({ ignoreCache: true });
    } catch (err) {
      console.warn('Failed to reload page:', err?.message ?? err);
    }
  }

  Console.messageAdded(({ message }) => {
    const text = message && (message.text || message.parameters && formatArgs(message.parameters));
    console.log(`[Console] ${message.level || 'log'}: ${text}`);
  });

  Runtime.consoleAPICalled(({ type, args }) => {
    console.log(`[Runtime] ${type}: ${formatArgs(args)}`);
  });

  Log.entryAdded(({ entry }) => {
    console.log(`[Log] ${entry.source}:${entry.level}: ${entry.text}`);
  });

  // Probe message to verify wiring without affecting app state
  try {
    await Runtime.evaluate({ expression: 'console.log("[Codex probe] console attached")', includeCommandLineAPI: true });
  } catch {}

  console.log(`Listening for console messages for ${(duration / 1000).toFixed(1)}s...`);
  await new Promise((r) => setTimeout(r, duration));
  await client.close();
  console.log('Done.');
})().catch((err) => {
  console.error('Error:', err && err.message ? err.message : err);
  process.exit(1);
});
