'use server';

import { createBeach } from '@/lib/api';
import { resolveManagerBaseUrl, resolveManagerToken } from '@/lib/serverSecrets';
import { safeAuth } from '@/lib/serverAuth';

type Result =
  | { success: true; id: string }
  | { success: false; error: string };

export async function createBeachAction(input: { name: string; slug?: string }) : Promise<Result> {
  const { getToken } = safeAuth();
  const template = process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE;
  const { token, source } = await resolveManagerToken(typeof getToken === 'function' ? getToken : undefined, template);
  const managerBaseUrl = resolveManagerBaseUrl();

  if (!token) {
    const reason = source === 'none' ? 'Manager token not configured.' : 'Unable to resolve Clerk token.';
    return { success: false, error: reason };
  }

  const sanitizedName = input.name.trim() || 'Private Beach';
  const sanitizedSlug = input.slug?.trim() || undefined;

  try {
    const created = await createBeach(sanitizedName, sanitizedSlug, token, managerBaseUrl);
    return { success: true, id: created.id };
  } catch (error) {
    const message = error instanceof Error ? error.message : 'Unable to create beach.';
    return { success: false, error: message };
  }
}
