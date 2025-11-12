import { clerkMiddleware } from '@clerk/nextjs/server';
import { NextResponse } from 'next/server';
import type { NextRequest, NextFetchEvent } from 'next/server';

// Allow unauthenticated access in test mode
const isTestMode = process.env.BEACH_TEST_MODE === 'true';

export default function middleware(request: NextRequest, event: NextFetchEvent) {
  // Bypass all auth in test mode
  if (isTestMode) {
    return NextResponse.next();
  }
  return clerkMiddleware()(request, event);
}

export const config = {
  matcher: [
    '/((?!_next|favicon.ico|sign-in|sign-up|api|static|.*\\..*).*)',
  ],
};
