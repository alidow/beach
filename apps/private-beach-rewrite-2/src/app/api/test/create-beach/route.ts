import { NextResponse } from 'next/server';

type CreateResponse = { id: string; name?: string };

export async function POST(request: Request) {
  const token =
    request.headers.get('x-pb-manager-token') ||
    request.headers.get('cookie')?.split(';').map((c) => c.trim()).find((c) => c.startsWith('pb-manager-token='))?.split('=')[1];

  if (!token) {
    return NextResponse.json({ error: 'missing manager token' }, { status: 401 });
  }

  const managerUrl =
    process.env.PRIVATE_BEACH_MANAGER_URL ||
    process.env.NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_URL ||
    process.env.NEXT_PUBLIC_MANAGER_URL ||
    'http://localhost:8080';

  const body = await request.json().catch(() => ({ name: 'Pong Showcase (test)' }));
  const payload = { name: body?.name || 'Pong Showcase (test)' };

  const resp = await fetch(`${managerUrl.replace(/\/$/, '')}/private-beaches`, {
    method: 'POST',
    headers: {
      authorization: `Bearer ${token}`,
      'content-type': 'application/json',
    },
    body: JSON.stringify(payload),
  }).catch(() => null);

  if (!resp || !resp.ok) {
    const detail = resp ? await resp.text().catch(() => resp.statusText) : 'unreachable';
    return NextResponse.json({ error: 'manager request failed', detail }, { status: 502 });
  }

  const data = (await resp.json().catch(() => ({}))) as CreateResponse;
  if (!data.id) {
    return NextResponse.json({ error: 'missing id in manager response' }, { status: 500 });
  }

  return NextResponse.json({ id: data.id, name: data.name }, { status: 200 });
}
