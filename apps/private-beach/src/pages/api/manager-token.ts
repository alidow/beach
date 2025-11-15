import type { NextApiRequest, NextApiResponse } from 'next';
import { getAuth } from '@clerk/nextjs/server';
import { resolveManagerToken } from '../../lib/serverSecrets';

export default async function handler(req: NextApiRequest, res: NextApiResponse) {
  if (req.method !== 'GET') {
    res.setHeader('Allow', ['GET']);
    res.status(405).json({ error: 'method_not_allowed' });
    return;
  }

  const auth = getAuth(req);
  const template = process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE;
  const allowedGetToken = typeof auth.getToken === 'function' ? auth.getToken : undefined;

  try {
    const { token, source, detail } = await resolveManagerToken(allowedGetToken, template, {
      isAuthenticated: Boolean(auth.userId),
    });
    if (!token) {
      let status = 500;
      if (source === 'unauthenticated') {
        status = 401;
      } else if (source === 'exchange_error') {
        status = 502;
      }
      res.status(status).json({ error: 'manager_token_unavailable', source, detail });
      return;
    }

    res.setHeader('Cache-Control', 'no-store');
    res.status(200).json({ token });
  } catch (error: any) {
    const message = error?.message ?? String(error);
    res.status(500).json({ error: 'manager_token_failure', detail: message });
  }
}
