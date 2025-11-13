import { NextResponse } from 'next/server';
import { safeAuth } from '@/lib/serverAuth';
import { resolveManagerToken } from '@/lib/serverSecrets';

export async function GET() {
  const { userId, getToken } = await safeAuth();
  const template = process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE;
  const allowedGetToken = typeof getToken === 'function' ? getToken : undefined;

  const { token, source, detail } = await resolveManagerToken(allowedGetToken, template, {
    isAuthenticated: Boolean(userId),
  });
  if (!token) {
    let status = 500;
    if (source === 'unauthenticated') {
      status = 401;
    } else if (source === 'exchange_error') {
      status = 502;
    }
    return NextResponse.json({ error: 'manager_token_unavailable', source, detail }, { status });
  }

  return NextResponse.json({ token }, { status: 200, headers: { 'cache-control': 'no-store' } });
}
