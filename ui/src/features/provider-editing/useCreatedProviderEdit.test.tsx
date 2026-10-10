import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, test, vi } from 'vitest';
import { useReaderHistoryStore } from '../reader-history/store';
import { useCanonicalHistory } from '../reader-history/useCanonicalHistory';
import { useCreatedProviderEdit } from './useCreatedProviderEdit';
import type { ProviderReaderReference } from '../../platform/providers';
import { useProviderEdits } from './store';
import { useEditorStore } from '../../stores/editor';
import { currentCreatedDocument } from './currentCreatedDocument';
const target: ProviderReaderReference = { sourceId: `provider:${'a'.repeat(64)}`, path: `records/${'b'.repeat(64)}.md`, revision: 'c'.repeat(64), title: 'Created.md', line: 1 };
const projection = (revision = target.revision) => ({ jobId: 'public-job', readerSourceId: target.sourceId, path: target.path, revision });
function backend() {
  return {
    state: vi.fn(async () => ({ sources: [{ sourceId: 'saved-public', readerSourceId: target.sourceId, name: 'Public Drive', kind: 'google_drive' as const, enabled: true, health: 'ready' as const,
      selectedScope: { kind: 'drive_folder' as const, folderId: 'root', recursive: true }, capabilities: { read: true, list: true, changes: true, assets: false, guardedWrite: true }, coverage: { kind: 'never_indexed' as const } }], availability: [], pendingAdmissions: [], error: null })),
    documents: vi.fn(async (_source: string, offset: number, _limit: number) => ({ operation: 'documents' as const, offset, nextOffset: null,
      documents: [{ documentId: 'native-created-id', name: 'Created.md', displayPath: 'Created.md', modifiedAtMs: null, sizeBytes: 0, cached: true, cachedObservedAtMs: 1, admitted: true, projection: projection() }] })),
    fetchCurrent: vi.fn(async (_source: string, _id: string) => projection()),
  };
}
beforeEach(() => { useReaderHistoryStore.getState().reset(); useProviderEdits.setState({ edits: {} }); useEditorStore.getState().reset(); });
test('creation validates current native head before editing and retargets only the same created tab', async () => {
  const api = backend(); api.fetchCurrent.mockResolvedValue(projection('d'.repeat(64)));
  const edit = vi.fn().mockResolvedValue(undefined), refresh = vi.fn(), notify = vi.fn();
  const view = renderHook(ready => { const history = useCanonicalHistory(); return { history, created: useCreatedProviderEdit(history.reference, ready, edit, history.setReference, refresh, notify, api) }; }, { initialProps: false });
  act(() => view.result.current.created(target));
  const tabId = useReaderHistoryStore.getState().data.activeTabId;
  await waitFor(() => expect(view.result.current.history.reference?.revision).toBe('d'.repeat(64)));
  expect(api.fetchCurrent).toHaveBeenCalledExactlyOnceWith('saved-public', 'native-created-id');
  expect(useReaderHistoryStore.getState().data.tabs).toHaveLength(1);
  expect(useReaderHistoryStore.getState().data.activeTabId).toBe(tabId);
  expect(useReaderHistoryStore.getState().data.tabs[0]!.entries[0]!.reference.revision).toBe(target.revision);
  expect(refresh).toHaveBeenCalledOnce(); expect(edit).not.toHaveBeenCalled();
  view.rerender(true);
  await waitFor(() => expect(edit).toHaveBeenCalledExactlyOnceWith(view.result.current.history.reference));
  expect(notify).not.toHaveBeenCalled();
});
test('selection changes cancel current-head reply and deferred editing', async () => {
  const api = backend(); let finish!: (value: ReturnType<typeof projection>) => void;
  api.fetchCurrent.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const edit = vi.fn(), notify = vi.fn();
  const view = renderHook(() => { const history = useCanonicalHistory(); return { history, created: useCreatedProviderEdit(history.reference, true, edit, history.setReference, vi.fn(), notify, api) }; });
  act(() => view.result.current.created(target));
  await waitFor(() => expect(api.fetchCurrent).toHaveBeenCalledOnce());
  act(() => view.result.current.history.setReference({ ...target, revision: 'e'.repeat(64) }));
  await act(async () => finish(projection('d'.repeat(64))));
  expect(view.result.current.history.reference?.revision).toBe('e'.repeat(64));
  expect(edit).not.toHaveBeenCalled(); expect(notify).not.toHaveBeenCalled();
});
test('only exact source/path projection maps a created document; same filename is never fetched', async () => {
  const api = backend(); const page = await api.documents('saved-public', 0, 256);
  api.documents.mockResolvedValue({ ...page, documents: [{ ...page.documents[0]!, projection: { ...projection(), path: `records/${'e'.repeat(64)}.md` } }] });
  await expect(currentCreatedDocument(target, api, () => true)).rejects.toThrow('current copy is unavailable');
  expect(api.fetchCurrent).not.toHaveBeenCalled();
});
test('wrong native current projection fails closed instead of editing another document', async () => {
  const api = backend(); api.fetchCurrent.mockResolvedValue({ ...projection(), readerSourceId: `provider:${'e'.repeat(64)}` });
  const edit = vi.fn(), notify = vi.fn();
  const view = renderHook(() => { const history = useCanonicalHistory(); return { history, created: useCreatedProviderEdit(history.reference, true, edit, history.setReference, vi.fn(), notify, api) }; });
  act(() => view.result.current.created(target));
  await waitFor(() => expect(notify).toHaveBeenCalledOnce());
  expect(view.result.current.history.reference?.revision).toBe(target.revision);
  expect(edit).not.toHaveBeenCalled();
});
test('unmount fences a late current-head reply', async () => {
  const api = backend(); let finish!: (value: ReturnType<typeof projection>) => void;
  api.fetchCurrent.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const edit = vi.fn(), notify = vi.fn();
  const view = renderHook(() => { const history = useCanonicalHistory(); return useCreatedProviderEdit(history.reference, true, edit, history.setReference, vi.fn(), notify, api); });
  act(() => view.result.current(target)); await waitFor(() => expect(api.fetchCurrent).toHaveBeenCalledOnce());
  const data = useReaderHistoryStore.getState().data; view.unmount();
  await act(async () => finish(projection('d'.repeat(64))));
  expect(useReaderHistoryStore.getState().data).toBe(data); expect(edit).not.toHaveBeenCalled(); expect(notify).not.toHaveBeenCalled();
});

test('unchanged current head still completes automatic editing once', async () => {
  const api = backend(), edit = vi.fn().mockResolvedValue(undefined);
  const view = renderHook(() => { const history = useCanonicalHistory(); return useCreatedProviderEdit(history.reference, true, edit, history.setReference, vi.fn(), vi.fn(), api); });
  act(() => view.result.current(target));
  await waitFor(() => expect(edit).toHaveBeenCalledOnce());
  expect(useReaderHistoryStore.getState().data.tabs).toHaveLength(1);
});
test('a retained dirty editor is preserved without fetching or automatic editing', async () => {
  const api = backend(), edit = vi.fn(), notify = vi.fn();
  useReaderHistoryStore.getState().open({ kind: 'provider', sourceId: target.sourceId, path: target.path, revision: target.revision }, 'Created.md');
  const tabId = `reader:${useReaderHistoryStore.getState().data.activeTabId}`;
  useEditorStore.getState().seed(tabId, 'retained-edit', '', target.revision, 'source');
  useEditorStore.getState().edit(tabId, 'retained-edit', 'Public unsaved draft');
  useProviderEdits.setState({ edits: { 'retained-edit': { key: 'retained-edit', tabId, reference: target,
    ticket: { operation: 'begin_edit_exact', writeSourceId: 'saved-public', editId: 'retained-edit', markdown: '', expiresAtMs: 1000, projection: projection() },
    phase: 'ready', pendingBody: null, projection: null, error: null } } });
  const view = renderHook(() => { const history = useCanonicalHistory(); return useCreatedProviderEdit(history.reference, true, edit, history.setReference, vi.fn(), notify, api); });
  act(() => view.result.current(target));
  expect(notify).toHaveBeenCalledOnce(); expect(api.fetchCurrent).not.toHaveBeenCalled(); expect(api.state).not.toHaveBeenCalled(); expect(edit).not.toHaveBeenCalled();
  expect(useEditorStore.getState().entry(tabId, 'retained-edit')?.buffer).toBe('Public unsaved draft');
});
