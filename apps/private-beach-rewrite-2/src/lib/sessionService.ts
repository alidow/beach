import { listSessions, type SessionSummary } from '@/lib/api';

type SessionServiceConfig = {
  token: string | null;
  baseUrl?: string;
};

const DEFAULT_CONFIG: SessionServiceConfig = {
  token: process.env.PRIVATE_BEACH_MANAGER_TOKEN ?? process.env.PRIVATE_BEACH_MANAGER_JWT ?? null,
  baseUrl: process.env.PRIVATE_BEACH_MANAGER_URL
};

export async function fetchSessions(
  privateBeachId: string,
  config: Partial<SessionServiceConfig> = {}
): Promise<SessionSummary[]> {
  const resolvedConfig = {
    ...DEFAULT_CONFIG,
    ...config
  };

  if (!resolvedConfig.token) {
    throw new Error('Missing PRIVATE_BEACH_MANAGER_TOKEN for session lookup');
  }

  return listSessions(privateBeachId, resolvedConfig.token, resolvedConfig.baseUrl);
}
