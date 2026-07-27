import { afterEach, describe, expect, test, vi } from 'vitest';

const persistence = vi.hoisted(() => ({
  raw: null as unknown,
  saves: 0,
}));

vi.mock('@tauri-apps/plugin-store', () => ({
  load: async () => ({
    get: async () => persistence.raw,
    set: async (_key: string, value: unknown) => {
      persistence.raw = value;
    },
    save: async () => {
      persistence.saves += 1;
    },
  }),
}));

import {
  readOfflineDocument,
  writeOfflineDocument,
  type MobileDocumentIdentity,
  type OfflineDocumentEntry,
} from './offline-cache';

function entry(
  sourceId: string,
  revision: string,
  cachedAt: number,
  overrides: Partial<OfflineDocumentEntry> = {},
): OfflineDocumentEntry {
  return {
    source: { kind: 'device-folder', id: sourceId },
    sourceRoot: { id: 'root-1', version: 'grant-1' },
    document: { id: 'document-1', path: '/notes/safe.md' },
    revision,
    observedAt: cachedAt - 10,
    cachedAt,
    freshUntil: cachedAt + 100,
    expiresAt: cachedAt + 1_000,
    content: '# Safe',
    bytes: 6,
    ...overrides,
  };
}

function identity(
  sourceId: string,
  overrides: Partial<MobileDocumentIdentity> = {},
): MobileDocumentIdentity {
  return {
    source: { kind: 'device-folder', id: sourceId },
    sourceRoot: { id: 'root-1', version: 'grant-1' },
    document: { id: 'document-1', path: '/notes/safe.md' },
    ...overrides,
  };
}

afterEach(() => {
  persistence.raw = null;
  persistence.saves = 0;
});

describe('bounded mobile offline cache', () => {
  test('a late older write cannot replace a newer observed revision', async () => {
    persistence.raw = {
      version: 2,
      entries: [entry('source-1', 'blake3:new', 2_000)],
    };

    expect(await writeOfflineDocument(entry('source-1', 'blake3:old', 1_000))).toBe(false);

    const stored = persistence.raw as { entries: OfflineDocumentEntry[] };
    expect(stored.entries).toHaveLength(1);
    expect(stored.entries[0]!.revision).toBe('blake3:new');
    expect(persistence.saves).toBe(0);
  });

  test('a revoked source identity cannot read a same-path entry after another source is admitted', async () => {
    persistence.raw = {
      version: 2,
      entries: [entry('revoked-source', 'blake3:a', 1_000)],
    };

    expect(await readOfflineDocument(identity('replacement-source'), 1_050)).toEqual({
      status: 'miss',
    });
  });

  test('a re-grant with a new root version cannot read the prior grant cache entry', async () => {
    persistence.raw = {
      version: 2,
      entries: [entry('source-1', 'blake3:a', 1_000)],
    };

    expect(
      await readOfflineDocument(
        identity('source-1', { sourceRoot: { id: 'root-1', version: 'grant-2' } }),
        1_050,
      ),
    ).toEqual({ status: 'miss' });
  });

  test('concurrent writes serialize the complete transaction without losing either document', async () => {
    const first = entry('source-1', 'blake3:first', 1_000);
    const second = entry('source-1', 'blake3:second', 1_100, {
      document: { id: 'document-2', path: '/notes/second.md' },
    });

    expect(await Promise.all([writeOfflineDocument(first), writeOfflineDocument(second)])).toEqual([
      true,
      true,
    ]);

    const stored = persistence.raw as { entries: OfflineDocumentEntry[] };
    expect(stored.entries.map((candidate) => candidate.document.id).sort()).toEqual([
      'document-1',
      'document-2',
    ]);
    expect(persistence.saves).toBe(2);
  });

  test('source kind and document id are part of the cache key even when ids and paths collide', async () => {
    persistence.raw = {
      version: 2,
      entries: [entry('shared-id', 'blake3:a', 1_000)],
    };

    expect(
      await readOfflineDocument(
        identity('shared-id', {
          source: { kind: 'dropbox', id: 'shared-id' },
        }),
        1_050,
      ),
    ).toEqual({ status: 'miss' });
    expect(
      await readOfflineDocument(
        identity('shared-id', {
          document: { id: 'document-2', path: '/notes/safe.md' },
        }),
        1_050,
      ),
    ).toEqual({ status: 'miss' });
  });
});
