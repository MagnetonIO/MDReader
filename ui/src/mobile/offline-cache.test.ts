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
    sourceRoot: '/notes',
    documentPath: '/notes/safe.md',
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

afterEach(() => {
  persistence.raw = null;
  persistence.saves = 0;
});

describe('bounded mobile offline cache', () => {
  test('a late older write cannot replace a newer observed revision', async () => {
    persistence.raw = {
      version: 1,
      entries: [entry('source-1', 'blake3:new', 2_000)],
    };

    expect(await writeOfflineDocument(entry('source-1', 'blake3:old', 1_000))).toBe(false);

    const stored = persistence.raw as { entries: OfflineDocumentEntry[] };
    expect(stored.entries).toHaveLength(1);
    expect(stored.entries[0]!.revision).toBe('blake3:new');
    expect(persistence.saves).toBe(0);
  });

  test('a path collision across source identities never returns an arbitrary document', async () => {
    persistence.raw = {
      version: 1,
      entries: [
        entry('source-1', 'blake3:a', 1_000),
        entry('source-2', 'blake3:b', 1_100),
      ],
    };

    expect(await readOfflineDocument('/notes', '/notes/safe.md', 1_150)).toEqual({
      status: 'corrupt',
    });
  });
});
