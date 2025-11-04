import { auth } from '@clerk/nextjs/server';

type AuthResult = ReturnType<typeof auth>;

/**
 * Wraps Clerk's `auth()` to avoid crashing when middleware is not registered.
 * For local/dev usage we fall back to an unauthenticated stub.
 */
export function safeAuth(): AuthResult {
  try {
    return auth();
  } catch (error) {
    if (process.env.NODE_ENV !== 'production') {
      console.warn('[private-beach-rewrite] Clerk auth unavailable, continuing without session', error);
    }
    const fallback: AuthResult = {
      userId: null,
      sessionId: null,
      actor: null,
      session: null,
      user: null,
      organization: null,
      orgId: null,
      orgRole: null,
      orgSlug: null,
      orgPermissions: null,
      has: () => false,
      debug: () => undefined,
      claims: {},
      getToken: async () => null,
    };
    return fallback;
  }
}
