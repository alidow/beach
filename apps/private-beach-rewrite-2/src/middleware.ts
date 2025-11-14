import { clerkMiddleware } from '@clerk/nextjs/server';

export default clerkMiddleware();

export const config = {
  matcher: [
    '/((?!_next|favicon.ico|sign-in|sign-up|api|static|.*\\..*).*)',
    // Manager token API must run through Clerk to exchange session tokens.
    '/api/manager-token',
  ],
};
