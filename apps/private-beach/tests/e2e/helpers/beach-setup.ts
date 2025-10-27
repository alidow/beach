/**
 * Create a new beach via the beach-manager API
 */
export async function createBeach(
  token: string,
  name: string,
  slug?: string,
  managerUrl: string = 'http://localhost:8080'
): Promise<{ id: string; name: string; slug: string }> {
  console.log(`Creating beach: ${name} (slug: ${slug || 'auto'})`);

  const body: { name: string; slug?: string } = { name };
  if (slug) {
    body.slug = slug;
  }

  const response = await fetch(`${managerUrl}/private-beaches`, {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${token}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });

  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(
      `Failed to create beach: ${response.status} ${response.statusText}\n${errorText}`
    );
  }

  const beach = await response.json();
  console.log(`Beach created: ${beach.id} (${beach.slug})`);

  return beach;
}

/**
 * Attach a session to a beach using session ID and passcode
 */
export async function attachSessionToBeach(
  token: string,
  beachId: string,
  sessionId: string,
  passcode: string,
  managerUrl: string = 'http://localhost:8080'
): Promise<void> {
  console.log(`Attaching session ${sessionId} to beach ${beachId}`);

  const response = await fetch(
    `${managerUrl}/private-beaches/${beachId}/sessions/attach-by-code`,
    {
      method: 'POST',
      headers: {
        'Authorization': `Bearer ${token}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        session_id: sessionId,
        code: passcode,
      }),
    }
  );

  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(
      `Failed to attach session: ${response.status} ${response.statusText}\n${errorText}`
    );
  }

  console.log('Session attached successfully');
}

/**
 * Delete a beach
 */
export async function deleteBeach(
  token: string,
  beachId: string,
  managerUrl: string = 'http://localhost:8080'
): Promise<void> {
  console.log(`Deleting beach ${beachId}`);

  const response = await fetch(`${managerUrl}/private-beaches/${beachId}`, {
    method: 'DELETE',
    headers: {
      'Authorization': `Bearer ${token}`,
      'Content-Type': 'application/json',
    },
  });

  if (!response.ok) {
    const errorText = await response.text();
    console.warn(
      `Failed to delete beach: ${response.status} ${response.statusText}\n${errorText}`
    );
    return;
  }

  console.log('Beach deleted successfully');
}

/**
 * Get session metadata from environment variables
 */
export function getSessionMetadata(): {
  sessionId: string;
  passcode: string;
  sessionServer: string;
} {
  const sessionId = process.env.BEACH_TEST_SESSION_ID;
  const passcode = process.env.BEACH_TEST_PASSCODE;
  const sessionServer =
    process.env.BEACH_TEST_SESSION_SERVER || 'http://localhost:4132';

  if (!sessionId || !passcode) {
    throw new Error(
      'Missing session credentials. Ensure BEACH_TEST_SESSION_ID and BEACH_TEST_PASSCODE are set.'
    );
  }

  return { sessionId, passcode, sessionServer };
}
