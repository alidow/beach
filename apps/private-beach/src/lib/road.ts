export type RoadMySession = {
  origin_session_id: string;
  kind: string;
  title?: string | null;
  started_at: number;
  last_seen_at: number;
  location_hint?: string | null;
};

function baseRoad(baseUrl?: string) {
  if (baseUrl) return baseUrl;
  return process.env.NEXT_PUBLIC_ROAD_URL || process.env.NEXT_PUBLIC_SESSION_SERVER_URL || 'https://api.beach.sh';
}

export async function listMySessions(token: string | null, roadUrl?: string): Promise<RoadMySession[]> {
  if (!token || token.trim().length === 0) {
    throw new Error('missing manager auth token for road request');
  }
  const effective = token.trim();
  const res = await fetch(`${baseRoad(roadUrl)}/me/sessions?status=active`, {
    headers: {
      'content-type': 'application/json',
      authorization: `Bearer ${effective}`,
    },
  });
  if (!res.ok) throw new Error(`road listMySessions failed ${res.status}`);
  return res.json();
}

export async function sendControlMessage(
  sessionId: string,
  kind: string,
  payload: any,
  token: string | null,
  roadUrl?: string,
): Promise<{ control_id: string }> {
  if (!token || token.trim().length === 0) {
    throw new Error('missing manager auth token for road control');
  }
  const res = await fetch(`${baseRoad(roadUrl)}/sessions/${sessionId}/control`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      authorization: `Bearer ${token.trim()}`,
    },
    body: JSON.stringify({ kind, payload }),
  });
  if (!res.ok) throw new Error(`road sendControlMessage failed ${res.status}`);
  const data = await res.json();
  return { control_id: data.control_id };
}

export async function pollControl(
  sessionId: string,
  code: string,
  roadUrl?: string,
): Promise<{ messages: any[] }> {
  const res = await fetch(`${baseRoad(roadUrl)}/sessions/${sessionId}/control/poll`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ code }),
  });
  if (!res.ok) throw new Error(`road pollControl failed ${res.status}`);
  return res.json();
}
