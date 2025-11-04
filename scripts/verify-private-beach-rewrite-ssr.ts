import http from 'node:http';

import { resolveManagerBaseUrl, resolveManagerToken } from '../apps/private-beach-rewrite/src/lib/serverSecrets';
import { listBeaches, getBeachMeta, listSessions } from '../apps/private-beach/src/lib/api';

function startMockManager(port: number) {
  const beaches = [
    { id: 'demo-beach', name: 'Demo Beach', created_at: Date.now() },
    { id: 'staging-beach', name: 'Staging Beach', created_at: Date.now() - 5_000 },
  ];
  const sessions = [
    {
      session_id: 'demo-session',
      private_beach_id: 'demo-beach',
      harness_type: 'application',
      capabilities: ['terminal'],
      version: '1',
      harness_id: 'demo',
      pending_actions: 0,
      pending_unacked: 0,
    },
  ];

  const server = http.createServer((req, res) => {
    if (!req.url) {
      res.statusCode = 404;
      res.end();
      return;
    }
    if (!req.headers.authorization?.startsWith('Bearer ')) {
      res.statusCode = 401;
      res.end();
      return;
    }
    if (req.url === '/private-beaches') {
      res.setHeader('content-type', 'application/json');
      res.end(JSON.stringify(beaches));
      return;
    }
    if (req.url === '/private-beaches/demo-beach') {
      res.setHeader('content-type', 'application/json');
      res.end(JSON.stringify({ ...beaches[0], slug: 'demo' }));
      return;
    }
    if (req.url === '/private-beaches/demo-beach/sessions') {
      res.setHeader('content-type', 'application/json');
      res.end(JSON.stringify(sessions));
      return;
    }
    res.statusCode = 404;
    res.end();
  });

  return new Promise<http.Server>((resolve) => {
    server.listen(port, () => resolve(server));
  });
}

async function main() {
  const port = 8787;
  const server = await startMockManager(port);

  process.env.PRIVATE_BEACH_MANAGER_TOKEN = 'mock-token';
  process.env.PRIVATE_BEACH_MANAGER_URL = `http://localhost:${port}`;

  try {
    const { token } = await resolveManagerToken(undefined, undefined);
    if (!token) {
      throw new Error('Token resolution failed');
    }
    const baseUrl = resolveManagerBaseUrl();
    const beaches = await listBeaches(token, baseUrl);
    if (beaches.length === 0) {
      throw new Error('Expected at least one beach from manager');
    }
    const beach = await getBeachMeta('demo-beach', token, baseUrl);
    const beachSessions = await listSessions('demo-beach', token, baseUrl);

    console.info(
      JSON.stringify(
        {
          ok: true,
          beachCount: beaches.length,
          beachName: beach.name,
          sessionCount: beachSessions.length,
        },
        null,
        2,
      ),
    );
  } finally {
    server.close();
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
