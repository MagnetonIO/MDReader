import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';

import type { SchemaSummary, SemanticsAdapter } from '../../platform/semantics.js';
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
    queryFederatedDocument: async () => {
      throw new Error('queryFederatedDocument is outside this schema-listing test');
    },
    cancelFederatedDocument: async () => {},
  };
}

describe('useSchemas', () => {
  test('reloads the restored lens with its schema and counts when that workspace becomes ready', async () => {
    // Removing readiness from the hook's dependencies recreates the persisted-lens
    // bug: this lens stays selected, but its count remains zero after indexing.
    const listSchemas = vi
      .fn<SemanticsAdapter['listSchemas']>()
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([READY_SCHEMA]);
    const adapter = adapterWithListSchemas(listSchemas);
    const { result, rerender } = renderHook(
      ({ workspaceKey, importPhase }) => {
        const listing = useSchemas(adapter, workspaceKey, true, importPhase === 'done');
        const restored = listing.schemas.find((schema) => schema.name === PERSISTED_LENS) ?? null;
        return { schemas: listing.schemas, restored };
      },
      {
        initialProps: {
          workspaceKey: '/workspace/restored',
          importPhase: 'loading' as 'loading' | 'done',
        },
      },
    );

    await waitFor(() => expect(result.current).toEqual({ schemas: [], restored: null }));

    rerender({ workspaceKey: '/workspace/restored', importPhase: 'done' });
    await waitFor(() => expect(result.current.restored).toEqual(READY_SCHEMA));
    expect(result.current.schemas).toEqual([READY_SCHEMA]);
    expect(result.current.restored).toMatchObject({
      objectCount: 7,
      proposalCount: 3,
    });
  });

  test('discards a late schema result from a retired workspace generation', async () => {
    const retiredWorkspace = deferred<SchemaSummary[]>();
    const listSchemas = vi
      .fn<SemanticsAdapter['listSchemas']>()
      .mockImplementationOnce(() => retiredWorkspace.promise)
      .mockResolvedValueOnce([READY_SCHEMA]);
    const adapter = adapterWithListSchemas(listSchemas);
    const { result, rerender } = renderHook(
      ({ workspaceKey }) => useSchemas(adapter, workspaceKey, true, true),
      { initialProps: { workspaceKey: '/workspace/retired' } },
    );

    rerender({ workspaceKey: '/workspace/current' });
    await waitFor(() => expect(result.current.schemas).toEqual([READY_SCHEMA]));

    await act(async () => {
      retiredWorkspace.resolve([]);
      await retiredWorkspace.promise;
    });
    expect(result.current.schemas).toEqual([READY_SCHEMA]);
  });
});
