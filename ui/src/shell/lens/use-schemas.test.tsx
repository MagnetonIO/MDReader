import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';

import type { ImportState } from '../../platform/fs.js';
import type { SchemaSummary, SemanticsAdapter } from '../../platform/semantics.js';
import { useSettingsStore } from '../../stores/settings.js';
import { useSourcesStore } from '../../stores/sources.js';
import { useSchemas } from './use-schemas.js';

const PERSISTED_LENS = 'decision_overlay';

const READY_SCHEMA: SchemaSummary = {
  name: PERSISTED_LENS,
  version: 4,
  checksum: 'blake3:ready',
  entities: ['Decision'],
  arrowCount: 1,
  attributeCount: 2,
  equationCount: 1,
  approvalRequired: true,
  objectCount: 7,
  proposalCount: 3,
};

const READY_IMPORT: ImportState = {
  phase: 'done',
  filesDone: 1,
  filesTotal: 1,
  chunks: 1,
  error: null,
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function adapterWithListSchemas(
  listSchemas: SemanticsAdapter['listSchemas'],
): SemanticsAdapter {
  return {
    listSchemas,
    projectCorpus: async () => {
      throw new Error('projectCorpus is outside this schema-listing test');
    },
    describeSchema: async () => {
      throw new Error('describeSchema is outside this schema-listing test');
    },
    inspectSpan: async () => {
      throw new Error('inspectSpan is outside this schema-listing test');
    },
    approveFact: async () => {
      throw new Error('approveFact is outside this schema-listing test');
    },
    rejectFact: async () => {
      throw new Error('rejectFact is outside this schema-listing test');
    },
    checkConstraints: async () => {
      throw new Error('checkConstraints is outside this schema-listing test');
    },
    explainQuery: async () => {
      throw new Error('explainQuery is outside this schema-listing test');
    },
  };
}

function usePersistedLens(adapter: SemanticsAdapter) {
  const workspaceKey = useSourcesStore((state) => state.root);
  const importState = useSourcesStore((state) => state.importState);
  const lensSchema = useSettingsStore((state) => state.lensSchema);
  const listing = useSchemas(adapter, workspaceKey, true, importState?.phase === 'done');
  const activeLens = listing.schemas.find((schema) => schema.name === lensSchema) ?? null;
  return { schemas: listing.schemas, activeLens };
}

afterEach(() => {
  cleanup();
  useSourcesStore.getState().reset();
  useSettingsStore.getState().reset();
  vi.clearAllMocks();
});

describe('useSchemas', () => {
  test('reloads a persisted lens exactly once with schema counts when its workspace becomes ready', async () => {
    // Removing readiness from the hook's dependencies recreates the production
    // break: the persisted lens stays selected but has no schema or counts after
    // the engine finishes indexing.
    const listSchemas = vi
      .fn<SemanticsAdapter['listSchemas']>()
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([READY_SCHEMA]);
    const adapter = adapterWithListSchemas(listSchemas);

    act(() => {
      useSourcesStore.getState().hydrate({ root: '/workspace/restored' });
      useSettingsStore.getState().hydrate({ lensSchema: PERSISTED_LENS });
    });
    const { result } = renderHook(() => usePersistedLens(adapter));

    await waitFor(() => expect(result.current).toEqual({ schemas: [], activeLens: null }));

    act(() => useSourcesStore.getState().setImport(READY_IMPORT));

    await waitFor(() => expect(result.current.activeLens).toEqual(READY_SCHEMA));
    expect(result.current.schemas).toEqual([READY_SCHEMA]);
    expect(result.current.activeLens).toMatchObject({ objectCount: 7, proposalCount: 3 });
    expect(listSchemas).toHaveBeenCalledTimes(2);
  });

  test('discards a late schema result from a retired workspace generation', async () => {
    const retiredWorkspace = deferred<SchemaSummary[]>();
    const listSchemas = vi
      .fn<SemanticsAdapter['listSchemas']>()
      .mockImplementationOnce(() => retiredWorkspace.promise)
      .mockResolvedValueOnce([READY_SCHEMA]);
    const adapter = adapterWithListSchemas(listSchemas);

    act(() => {
      useSourcesStore.getState().hydrate({ root: '/workspace/retired' });
      useSourcesStore.getState().setImport(READY_IMPORT);
      useSettingsStore.getState().hydrate({ lensSchema: PERSISTED_LENS });
    });
    const { result } = renderHook(() => usePersistedLens(adapter));

    act(() => useSourcesStore.getState().setRoot('/workspace/current'));
    await waitFor(() => expect(result.current.schemas).toEqual([READY_SCHEMA]));

    await act(async () => {
      retiredWorkspace.resolve([]);
      await retiredWorkspace.promise;
    });
    expect(result.current.schemas).toEqual([READY_SCHEMA]);
  });
});
