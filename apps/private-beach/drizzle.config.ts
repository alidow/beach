import 'dotenv/config';
import { defineConfig } from 'drizzle-kit';

const connectionString =
  process.env.PRIVATE_BEACH_DATABASE_URL ??
  process.env.DATABASE_URL ??
  '';

if (!connectionString) {
  throw new Error('PRIVATE_BEACH_DATABASE_URL (or DATABASE_URL) must be set for Drizzle config.');
}

export default defineConfig({
  schema: './src/db/schema.ts',
  out: './drizzle',
  dialect: 'postgresql',
  dbCredentials: {
    url: connectionString,
  },
});
