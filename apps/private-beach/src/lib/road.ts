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
