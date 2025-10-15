import { loadConfig } from './config.js';
import { buildServer } from './server.js';

async function main(): Promise<void> {
  const config = loadConfig();
  const server = await buildServer({ config });

  try {
    await server.listen({
      port: config.port,
      host: config.host,
    });
    server.log.info({ port: config.port, host: config.host }, 'beach-gate listening');
  } catch (error) {
    server.log.error(error, 'failed_to_start');
    process.exitCode = 1;
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  void main();
}
