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
  if (typeof window !== 'undefined') {
    const w = window as any;
    if (w.NEXT_PUBLIC_ROAD_URL) return w.NEXT_PUBLIC_ROAD_URL as string;
    const ls = localStorage.getItem('pb.road');
    if (ls) return ls;
  }
  return process.env.NEXT_PUBLIC_ROAD_URL || process.env.NEXT_PUBLIC_SESSION_SERVER_URL || 'http://localhost:4132';
}

export async function listMySessions(token: string | null, roadUrl?: string): Promise<RoadMySession[]> {
  const res = await fetch(`${baseRoad(roadUrl)}/me/sessions?status=active`, {
    headers: {
      'content-type': 'application/json',
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
  });
  if (!res.ok) throw new Error(`road listMySessions failed ${res.status}`);
  return res.json();
}

