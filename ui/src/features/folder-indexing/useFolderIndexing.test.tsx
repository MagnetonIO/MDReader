import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { useFolderIndexing } from './useFolderIndexing';
import type { FolderIndexStatus, FolderIndexingBackend } from '../../platform/folder-indexing';
const rows: FolderIndexStatus[] = ['first', 'second'].map(sourceId => ({ sourceId, monitorEnabled: true, phase: 'ready', knownDocuments: 4, indexedDocuments: 4, observedAtMs: 1, complete: true, error: null }));
const backend = (): FolderIndexingBackend => ({ state: vi.fn(async () => rows), setMonitoring: vi.fn<FolderIndexingBackend['setMonitoring']>(async (sourceId, enabled) => rows.map(row => row.sourceId === sourceId ? { ...row, monitorEnabled: enabled, phase: enabled ? 'ready' : 'paused' } : row)), pickBroadSource: vi.fn() });
afterEach(cleanup);
test('all saved-folder statuses are independent of the active reading folder; pause changes only the selected native id', async () => {
  const api = backend(), view = renderHook(() => useFolderIndexing(true, 'first|second', api));
  await waitFor(() => expect(view.result.current.rows).toHaveLength(2));
  await act(async () => { await view.result.current.setMonitoring('second', false); });
  expect(api.setMonitoring).toHaveBeenCalledExactlyOnceWith('second', false);
  expect(view.result.current.rows?.find(row => row.sourceId === 'first')?.monitorEnabled).toBe(true);
  expect(view.result.current.rows?.find(row => row.sourceId === 'second')?.phase).toBe('paused');
});
test('late folder status is fenced after the library category closes', async () => {
  const api = backend(); let resolve!: (value: FolderIndexStatus[]) => void;
  vi.mocked(api.state).mockReturnValue(new Promise(done => { resolve = done; }));
  const view = renderHook(({ active }) => useFolderIndexing(active, 'first|second', api), { initialProps: { active: true } });
  view.rerender({ active: false });
  await act(async () => resolve(rows));
  expect(view.result.current.rows).toBeNull(); expect(api.setMonitoring).not.toHaveBeenCalled();
});
