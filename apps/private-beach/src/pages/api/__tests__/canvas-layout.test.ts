import { describe, expect, test, vi, beforeEach } from 'vitest';
import type { NextApiRequest, NextApiResponse } from 'next';
import handler from '../canvas-layout/[id]';

const selectMock = vi.fn();
const insertMock = vi.fn();
const ensureMigratedMock = vi.fn(async () => {});

vi.mock('../../../db/client', () => ({
  db: {
    select: selectMock,
    insert: insertMock,
  },
  ensureMigrated: ensureMigratedMock,
}));

const sampleLayout = {
  version: 3 as const,
  viewport: { zoom: 1, pan: { x: 0, y: 0 } },
  tiles: {
    'session-1': {
      id: 'session-1',
      kind: 'application' as const,
      position: { x: 120, y: 80 },
      size: { width: 400, height: 320 },
      zIndex: 1,
    },
  },
  agents: {},
  groups: {},
  controlAssignments: {},
  metadata: { createdAt: 1, updatedAt: 1 },
};

function createMockResponse() {
  const json = vi.fn();
  const status = vi.fn(() => ({ json }));
  const res = { status } as unknown as NextApiResponse;
  return { res, status, json };
}

beforeEach(() => {
  vi.clearAllMocks();
  ensureMigratedMock.mockResolvedValue(undefined);
});

describe('canvas-layout API route', () => {
  test('GET returns stored layout when present', async () => {
    selectMock.mockImplementationOnce(() => ({
      from: () => ({
        where: () => ({
          limit: async () => [{ layout: sampleLayout }],
        }),
      }),
    }));

    const req = { method: 'GET', query: { id: 'beach-123' } } as unknown as NextApiRequest;
    const { res, status, json } = createMockResponse();

    await handler(req, res);

    expect(ensureMigratedMock).toHaveBeenCalled();
    expect(status).toHaveBeenCalledWith(200);
    expect(json).toHaveBeenCalledWith(sampleLayout);
  });

  test('GET falls back to empty layout when nothing stored', async () => {
    selectMock
      .mockImplementationOnce(() => ({
        from: () => ({
          where: () => ({
            limit: async () => [],
          }),
        }),
      }))
      .mockImplementationOnce(() => ({
        from: () => ({
          where: () => ({
            limit: async () => [],
          }),
        }),
      }));

    const req = { method: 'GET', query: { id: 'beach-456' } } as unknown as NextApiRequest;
    const { res, status, json } = createMockResponse();

    await handler(req, res);

    expect(status).toHaveBeenCalledWith(200);
    const payload = json.mock.calls[0]?.[0];
    expect(payload).toMatchObject({ version: 3, tiles: {}, agents: {}, groups: {} });
  });

  test('PUT validates and persists layout', async () => {
    const onConflictDoUpdate = vi.fn(async () => {});
    const values = vi.fn(() => ({ onConflictDoUpdate }));
    insertMock.mockImplementation(() => ({ values }));

    const req = {
      method: 'PUT',
      query: { id: 'beach-789' },
      body: sampleLayout,
    } as unknown as NextApiRequest;
    const { res, status, json } = createMockResponse();

    await handler(req, res);

    expect(status).toHaveBeenCalledWith(200);
    const responseLayout = json.mock.calls[0]?.[0];
    expect(responseLayout.version).toBe(3);
    expect(responseLayout.metadata.updatedAt).toBeGreaterThanOrEqual(sampleLayout.metadata.updatedAt);
    expect(values).toHaveBeenCalled();
    expect(onConflictDoUpdate).toHaveBeenCalled();
  });

  test('PUT rejects invalid payload', async () => {
    const req = {
      method: 'PUT',
      query: { id: 'beach-000' },
      body: { wrong: true },
    } as unknown as NextApiRequest;
    const { res, status, json } = createMockResponse();

    await handler(req, res);

    expect(status).toHaveBeenCalledWith(400);
    expect(json).toHaveBeenCalledWith({ error: 'invalid canvas layout' });
    expect(insertMock).not.toHaveBeenCalled();
  });
});
