'use server';

import { createBeach, deleteBeach } from '@/lib/api';
import { resolveManagerBaseUrl, resolveManagerToken } from '@/lib/serverSecrets';
import { safeAuth } from '@/lib/serverAuth';

type Result =
  | { success: true; id: string }
  | { success: false; error: string };

export async function createBeachAction(input: { name: string; slug?: string }) : Promise<Result> {
  const debug = (...args: unknown[]) => {
    if (process.env.NODE_ENV !== 'production') {
      // eslint-disable-next-line no-console
      console.log('[createBeachAction]', ...args);
    }
  };

  debug('received request', {
    nameLength: input.name?.length ?? 0,
    slugProvided: Boolean(input.slug?.trim()),
  });

  const { userId, getToken } = await safeAuth();
  const isSignedIn = Boolean(userId);
  const template = process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE;
  const allowedGetToken = typeof getToken === 'function' ? getToken : undefined;
  const { token, source } = await resolveManagerToken(allowedGetToken, template, {
    isAuthenticated: isSignedIn,
  });
  const managerBaseUrl = resolveManagerBaseUrl();

  if (!token) {
    let reason: string;
    if (source === 'unauthenticated') {
      reason = 'Sign in to create private beaches.';
    } else if (source === 'exchange_error') {
      reason = 'Unable to mint a Beach Gate token. Ensure Gate is reachable.';
    } else if (source === 'none') {
      reason = 'Manager token not configured.';
    } else {
      reason = 'Unable to resolve Clerk token.';
    }
    debug('missing token', { source, reason });
    return { success: false, error: reason };
  }

  const sanitizedName = input.name.trim() || 'Private Beach';
  const sanitizedSlug = input.slug?.trim() || undefined;

  debug('creating beach', {
    sanitizedNameLength: sanitizedName.length,
    slugProvided: Boolean(sanitizedSlug),
    managerBaseUrl,
  });

  try {
    const created = await createBeach(sanitizedName, sanitizedSlug, token, managerBaseUrl);
    debug('create succeeded', { id: created.id });
    return { success: true, id: created.id };
  } catch (error) {
    const message = error instanceof Error ? error.message : 'Unable to create beach.';
    debug('create failed', { message });
    return { success: false, error: message };
  }
}

type DeleteResult =
  | { success: true; deleted: string[] }
  | { success: false; error: string };

export async function deleteBeachesAction(input: { ids: string[] }): Promise<DeleteResult> {
  const debug = (...args: unknown[]) => {
    if (process.env.NODE_ENV !== 'production') {
      // eslint-disable-next-line no-console
      console.log('[deleteBeachesAction]', ...args);
    }
  };

  const sanitizedIds = Array.from(
    new Set(
      (input?.ids ?? [])
        .map((value) => (typeof value === 'string' ? value.trim() : ''))
        .filter((value) => value.length > 0),
    ),
  );
  if (sanitizedIds.length === 0) {
    return { success: false, error: 'Select at least one beach to delete.' };
  }

  debug('request received', { count: sanitizedIds.length });

  const { userId, getToken } = await safeAuth();
  const isSignedIn = Boolean(userId);
  const template = process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE;
  const allowedGetToken = typeof getToken === 'function' ? getToken : undefined;
  const { token, source } = await resolveManagerToken(allowedGetToken, template, {
    isAuthenticated: isSignedIn,
  });
  const managerBaseUrl = resolveManagerBaseUrl();

  if (!token) {
    let reason: string;
    if (source === 'unauthenticated') {
      reason = 'Sign in to delete private beaches.';
    } else if (source === 'exchange_error') {
      reason = 'Unable to mint a Beach Gate token. Ensure Gate is reachable.';
    } else if (source === 'none') {
      reason = 'Manager token not configured.';
    } else {
      reason = 'Unable to resolve Clerk token.';
    }
    debug('missing token', { source, reason });
    return { success: false, error: reason };
  }

  const deleted: string[] = [];
  try {
    for (const id of sanitizedIds) {
      debug('deleting beach', { id });
      await deleteBeach(id, token, managerBaseUrl);
      deleted.push(id);
    }
    debug('delete succeeded', { count: deleted.length });
    return { success: true, deleted };
  } catch (error) {
    const failedId = deleted.length < sanitizedIds.length ? sanitizedIds[deleted.length] : null;
    const message = error instanceof Error ? error.message : 'Unable to delete beaches.';
    debug('delete failed', { message, failedId });
    const reason = failedId ? `Failed to delete ${failedId}: ${message}` : message;
    return { success: false, error: reason };
  }
}
