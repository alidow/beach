import type { NextApiRequest, NextApiResponse } from 'next';
import { getAllViewerCounters, resetViewerCounters } from '../../../controllers/metricsRegistry';

export default function handler(req: NextApiRequest, res: NextApiResponse) {
  if (process.env.NODE_ENV === 'production') {
    res.status(404).json({ error: 'Not found' });
    return;
  }

  if (req.method === 'DELETE') {
    resetViewerCounters();
    res.status(204).end();
    return;
  }

  if (req.method === 'GET') {
    res.status(200).json({
      counters: getAllViewerCounters(),
    });
    return;
  }

  res.setHeader('Allow', ['GET', 'DELETE']);
  res.status(405).json({ error: 'Method not allowed' });
}
