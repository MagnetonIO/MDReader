import { expect, test, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { archivedReviewsBackend, ArchivedReviewsError } from './adapter';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
test('native comparison receives only the exact reference and typed failures stay redacted', async () => {
  const reference = { sourceId: `provider:${'a'.repeat(64)}`, path: 'records/selected.md', revision: 'b'.repeat(64), title: 'Display metadata', line: 8 };
  vi.mocked(invoke).mockRejectedValueOnce({ kind: 'conflict', privatePath: '/never-display' });
  await expect(archivedReviewsBackend.compare('handle', 'record', reference, null)).rejects.toEqual(new ArchivedReviewsError('conflict'));
  expect(invoke).toHaveBeenCalledWith('reader_archived_reviews', { request: { operation: 'compare', handle: 'handle', recordId: 'record', current: { sourceId: reference.sourceId, path: reference.path, revision: reference.revision }, generation: null } });
});
