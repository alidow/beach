import fs from 'fs';
import path from 'path';
import { drizzle } from 'drizzle-orm/node-postgres';
import { migrate } from 'drizzle-orm/node-postgres/migrator';
import { Pool } from 'pg';
import * as schema from './schema';

const connectionString =
  process.env.PRIVATE_BEACH_DATABASE_URL ??
  process.env.DATABASE_URL ??
  '';

if (!connectionString) {
  throw new Error('PRIVATE_BEACH_DATABASE_URL (or DATABASE_URL) must be set.');
}

const pool = new Pool({
  connectionString,
  max: 5,
});

pool.on('error', (err: Error) => {
  console.error('Unexpected Postgres pool error', err);
});

export const db = drizzle(pool, { schema });

function resolveMigrationsFolder(): string {
  const candidates = [
    path.join(process.cwd(), 'apps/private-beach/drizzle'),
    path.join(process.cwd(), 'drizzle'),
    path.join(__dirname, '../../drizzle'),
  ];
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return path.join(process.cwd(), 'apps/private-beach/drizzle');
}

const migrationsFolder = resolveMigrationsFolder();
let migrationPromise: Promise<void> | null = null;

export async function ensureMigrated(): Promise<void> {
  if (!migrationPromise) {
    migrationPromise = migrate(db, { migrationsFolder }).catch((err) => {
      migrationPromise = null;
      throw err;
    });
  }
  await migrationPromise;
}
