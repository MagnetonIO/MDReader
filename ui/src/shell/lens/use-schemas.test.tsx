import { renderHook, waitFor } from '@testing-library/react';
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

describe('useSchemas', () => {
  test('reloads a restored lens after readiness and discards a retired workspace result', async () => {
    // Removing readiness from the hook's dependencies recreates the persisted-lens
    // bug: this lens stays selected, but its count remains zero after indexing.
    const retiredWorkspace = deferred<SchemaSummary[]>();
    const listSchemas = vi
      .fn<SemanticsAdapter['listSchemas']>()
      .mockResolvedValueOnce([])
      .mockImplementationOnce(() => retiredWorkspace.promise)
      .mockImplementationOnce(async () => [READY_SCHEMA]);
    const adapter: SemanticsAdapter = {
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

    const { result, rerender } = renderHook(
      ({ workspaceKey, importPhase }) => {
        const listing = useSchemas(adapter, workspaceKey, true, importPhase === 'done');
        const restored = listing.schemas.find((schema) => schema.name === PERSISTED_LENS) ?? null;
        return restored === null
          ? { schemas: 0, graphObjects: 0, claims: 0 }
          : {
              schemas: listing.schemas.length,
              graphObjects: restored.objectCount,
              claims: restored.proposalCount,
            };
      },
      {
        initialProps: {
          workspaceKey: '/workspace/retired',
          importPhase: 'loading' as 'loading' | 'done',
        },
      },
    );

    await waitFor(() => expect(result.current).toEqual({ schemas: 0, graphObjects: 0, claims: 0 }));

    rerender({ workspaceKey: '/workspace/retired', importPhase: 'done' });
    await waitFor(() => expect(listSchemas).toHaveBeenCalledTimes(2));

    rerender({ workspaceKey: '/workspace/current', importPhase: 'done' });
    await waitFor(() => expect(result.current).toEqual({ schemas: 1, graphObjects: 7, claims: 3 }));

    retiredWorkspace.resolve([]);
    await Promise.resolve();
    expect(result.current).toEqual({ schemas: 1, graphObjects: 7, claims: 3 });
  });
});
