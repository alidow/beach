/**
 * Beach Session Fixture
 *
 * Manages the lifecycle of Beach terminal sessions for E2E tests.
 * Sessions are provided via environment variables (started externally by automation scripts)
 * or can be started/stopped programmatically.
 */

export interface BeachSessionCredentials {
  sessionId: string;
  passcode: string;
  sessionServer: string;
}

/**
 * Get Beach session credentials from environment variables
 *
 * Expected environment variables:
 * - BEACH_TEST_SESSION_ID
 * - BEACH_TEST_PASSCODE
 * - BEACH_TEST_SESSION_SERVER (default: http://localhost:8080)
 */
export function getSessionCredentialsFromEnv(): BeachSessionCredentials | null {
  const sessionId = process.env.BEACH_TEST_SESSION_ID;
  const passcode = process.env.BEACH_TEST_PASSCODE;
  const sessionServer = process.env.BEACH_TEST_SESSION_SERVER || 'http://localhost:8080';

  if (!sessionId || !passcode) {
    return null;
  }

  return {
    sessionId,
    passcode,
    sessionServer,
  };
}

/**
 * Validate that required session credentials are available
 *
 * Throws an error with helpful instructions if credentials are missing.
 */
export function requireSessionCredentials(): BeachSessionCredentials {
  const creds = getSessionCredentialsFromEnv();

  if (!creds) {
    throw new Error(`
❌ Beach session credentials not found in environment variables.

Please set the following environment variables:
  BEACH_TEST_SESSION_ID=<session-id>
  BEACH_TEST_PASSCODE=<passcode>
  BEACH_TEST_SESSION_SERVER=http://localhost:8080  (optional)

You can run the automated test script which sets these up automatically:
  cd apps/private-beach
  ./tests/scripts/run-tile-resize-test.sh

Or manually start a Beach session:
  cd apps/private-beach/demo/pong
  python3 tools/launch_session.py
    `);
  }

  return creds;
}

/**
 * Get the manager URL for Private Beach API calls
 */
export function getManagerUrl(): string {
  return process.env.BEACH_TEST_MANAGER_URL || 'http://localhost:8080';
}

/**
 * Get the Private Beach base URL
 */
export function getPrivateBeachUrl(): string {
  return process.env.BEACH_TEST_PRIVATE_BEACH_URL || 'http://localhost:3000';
}

/**
 * Session metadata for tracking in tests
 */
export interface SessionMetadata {
  credentials: BeachSessionCredentials;
  managerUrl: string;
  privateBeachUrl: string;
  started: Date;
}

/**
 * Create session metadata for a test
 */
export function createSessionMetadata(): SessionMetadata {
  const credentials = requireSessionCredentials();

  return {
    credentials,
    managerUrl: getManagerUrl(),
    privateBeachUrl: getPrivateBeachUrl(),
    started: new Date(),
  };
}
