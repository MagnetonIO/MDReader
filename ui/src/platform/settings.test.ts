import { describe, expect, it, vi } from 'vitest';

import {
  commands,
  type DiagnosticEvent,
  type LibraryEntityQueryResponse_Serialize,
} from '../bindings';
import {
  LIBRARY_LOGICAL_LIMIT,
  LIBRARY_REFRESH_WINDOW,
  queryLibraryDocuments,
  recordDiagnosticEventNonBlocking,
} from './settings.js';

describe('local diagnostics transport', () => {
  it('never rejects or stalls the product action that emitted the event', async () => {
    const event: DiagnosticEvent = { kind: 'application-started' };
    const write = vi.fn(async () => {
      throw new Error('disk full');
    });

    expect(() => recordDiagnosticEventNonBlocking(event, write)).not.toThrow();
    await Promise.resolve();

    expect(write).toHaveBeenCalledWith(event);
  });

  it('does not retain arbitrary payloads outside the closed event vocabulary', () => {
    const source = recordDiagnosticEventNonBlocking.toString();

    expect(source).not.toMatch(/query|document|content|path/i);
  });
});

describe('Library query transport', () => {
  it('requests one bounded sidebar window instead of an eager Library export', async () => {
    const response = {} as LibraryEntityQueryResponse_Serialize;
    const query = vi
      .spyOn(commands, 'queryLibraryDocuments')
      .mockResolvedValueOnce({ status: 'ok', data: response });

    await expect(
      queryLibraryDocuments(
        '00000000-0000-0000-0000-000000000001',
        [],
        ['title', 'path'],
        'library-window',
      ),
    ).resolves.toBe(response);

    expect(query).toHaveBeenCalledWith(
      'library-window',
      expect.objectContaining({
        page_size: LIBRARY_REFRESH_WINDOW,
        limit: LIBRARY_LOGICAL_LIMIT,
        query_id: null,
        cursor: null,
      }),
    );
  });

  it('passes only the typed application continuation back to the native Library', async () => {
    const response = {} as LibraryEntityQueryResponse_Serialize;
    const query = vi
      .spyOn(commands, 'queryLibraryDocuments')
      .mockResolvedValueOnce({ status: 'ok', data: response });

    await queryLibraryDocuments(
      '00000000-0000-0000-0000-000000000001',
      [],
      ['title', 'path'],
      'library-page-2',
      ['00000000-0000-0000-0000-000000000002'],
      {
        queryId: '00000000-0000-0000-0000-000000000003',
        cursor: 'v1.library.application-cursor',
      },
    );

    expect(query).toHaveBeenCalledWith(
      'library-page-2',
      expect.objectContaining({
        source_ids: ['00000000-0000-0000-0000-000000000002'],
        query_id: '00000000-0000-0000-0000-000000000003',
        cursor: 'v1.library.application-cursor',
        page_size: LIBRARY_REFRESH_WINDOW,
        limit: LIBRARY_LOGICAL_LIMIT,
      }),
    );
  });
});
