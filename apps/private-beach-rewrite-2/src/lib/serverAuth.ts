import { auth } from '@clerk/nextjs/server';

type AuthPromise = ReturnType<typeof auth>;
type AuthResult = Awaited<AuthPromise>;

/**
 * Wraps Clerk's `auth()` to avoid crashing when middleware is not registered.
 * For local/dev usage we fall back to an unauthenticated stub.
 */
export async function safeAuth(): Promise<AuthResult> {
  try {
    return await auth();
  } catch (error) {
    if (process.env.NODE_ENV !== 'production') {
      console.warn('[private-beach-rewrite-2] Clerk auth unavailable, continuing without session', error);
    }
    const fallback = {
      userId: null,
      sessionId: null,
      sessionClaims: null,
      sessionStatus: null,
      actor: null,
      tokenType: 'session_token',
      factorVerificationAge: null,
      isAuthenticated: false,
      session: null,
      user: null,
      organization: null,
      orgId: null,
      orgRole: null,
      orgSlug: null,
      orgPermissions: null,
      has: () => false,
      debug: () => ({}),
      claims: {},
      getToken: async () => null,
      redirectToSignIn: () => {
        throw new Error('redirectToSignIn is unavailable in safeAuth fallback');
      },
      redirectToSignUp: () => {
        throw new Error('redirectToSignUp is unavailable in safeAuth fallback');
      },
    } as unknown as AuthResult;
    return fallback;
  }
}
