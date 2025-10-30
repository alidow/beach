import type { NextApiRequest, NextApiResponse } from 'next';

export default function handler(_req: NextApiRequest, res: NextApiResponse) {
  res.status(410).json({
    error: 'legacy_layout_removed',
    message: 'The grid-based layout endpoint has been retired. Use /api/canvas-layout/:id or the manager /private-beaches/:id/layout endpoint for CanvasLayout v3.',
  });
}
