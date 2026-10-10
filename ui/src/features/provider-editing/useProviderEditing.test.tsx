import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { nativeProvidersBackend, type ProviderCoverage, type ProviderSource } from '../../platform/providers';
import { useProviderEditing } from './useProviderEditing';
import { providerEditBackend } from './backend';
import { useReaderHistoryStore } from '../reader-history/store';
import { useCanonicalHistory } from '../reader-history/useCanonicalHistory';
import { useProviderEdits } from './store';
import { useEditorStore } from '../../stores/editor';

const reference = { sourceId: `provider:${'a'.repeat(64)}`, path: `records/${'d'.repeat(64)}.md`, revision: 'b'.repeat(64) };
beforeEach(() => { useReaderHistoryStore.getState().reset(); useProviderEdits.setState({ edits: {} }); useEditorStore.getState().reset(); });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

test.each([
  [{ kind: 'never_indexed' }, true],
  [{ kind: 'partial', reason: 'metadata incomplete' }, true],
  [{ kind: 'metadata_complete_selected', observedAtMs: 1 }, true],
  [{ kind: 'complete_selected', observedAtMs: 1 }, true],
] satisfies [ProviderCoverage, boolean][])('offers an explicit exact edit for an authorized grant independently of whole-source discovery (%j)', async (coverage, expected) => {
  const source: ProviderSource = { sourceId: 'public-provider', readerSourceId: reference.sourceId, name: 'Public notes', kind: 'google_drive', enabled: true,
    selectedScope: { kind: 'drive_folder', folderId: 'public-root', recursive: true },
    capabilities: { read: true, list: true, changes: false, assets: false, guardedWrite: true }, health: 'ready', coverage };
  const state = vi.spyOn(nativeProvidersBackend, 'state').mockResolvedValue({ sources: [source], availability: [], pendingAdmissions: [], error: null });
  const view = renderHook(() => useProviderEditing(reference, 'public-tab', vi.fn()));
  await act(async () => { await Promise.resolve(); });
  expect(state).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(view.result.current.canEdit).toBe(expected));
  installLocalProjection();
  const begin = vi.spyOn(providerEditBackend, 'begin').mockRejectedValue(new Error('Native exact revision check refused'));
  expect(begin).not.toHaveBeenCalled();
  await act(async () => view.result.current.edit(reference));
  expect(begin).toHaveBeenCalledExactlyOnceWith(reference);
});

test('a new exact receipt rechecks editing availability and ignores a late reply for the prior document', async () => {
  const writable: ProviderSource = { sourceId: 'public-provider', readerSourceId: reference.sourceId, name: 'Public notes', kind: 'google_drive', enabled: true,
    selectedScope: { kind: 'drive_folder', folderId: 'public-root', recursive: true },
    capabilities: { read: true, list: true, changes: false, assets: false, guardedWrite: true }, health: 'ready',
    coverage: { kind: 'metadata_complete_selected', observedAtMs: 1 } };
  const snapshot = (allowed: boolean) => ({ sources: [{ ...writable, capabilities: { ...writable.capabilities, guardedWrite: allowed } }], availability: [], pendingAdmissions: [], error: null });
  let previous!: (value: ReturnType<typeof snapshot>) => void;
  let next!: (value: ReturnType<typeof snapshot>) => void;
  const state = vi.spyOn(nativeProvidersBackend, 'state')
    .mockImplementationOnce(() => new Promise(resolve => { previous = resolve; }))
    .mockImplementationOnce(() => new Promise(resolve => { next = resolve; }));
  const view = renderHook(value => useProviderEditing(value, 'public-tab', vi.fn()), { initialProps: reference });
  const newReference = { ...reference, path: 'records/second.md', revision: 'c'.repeat(64) };
  view.rerender(newReference);
  expect(view.result.current.canEdit).toBe(false);
  await act(async () => next(snapshot(true)));
  expect(view.result.current.canEdit).toBe(true);
  await act(async () => previous(snapshot(false)));
  expect(view.result.current.canEdit).toBe(true);
  expect(state).toHaveBeenCalledTimes(2);
});

test('a changed grant observation is fenced immediately until its new capability answer arrives', async () => {
  const source: ProviderSource = { sourceId: 'public-provider', readerSourceId: reference.sourceId, name: 'Public notes', kind: 'google_drive', enabled: true,
    selectedScope: { kind: 'drive_folder', folderId: 'root', recursive: true },
    capabilities: { read: true, list: true, changes: false, assets: false, guardedWrite: true }, health: 'ready',
    coverage: { kind: 'complete_selected', observedAtMs: 1 } };
  let finish!: (value: Awaited<ReturnType<typeof nativeProvidersBackend.state>>) => void;
  vi.spyOn(nativeProvidersBackend, 'state').mockResolvedValueOnce({ sources: [source], availability: [], pendingAdmissions: [], error: null })
    .mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const view = renderHook(key => useProviderEditing(reference, 'public-tab', vi.fn(), key), { initialProps: 0 });
  await waitFor(() => expect(view.result.current.canEdit).toBe(true));
  view.rerender(1);
  expect(view.result.current.canEdit).toBe(false);
  await act(async () => finish({ sources: [{ ...source, enabled: false, health: 'paused' }], availability: [], pendingAdmissions: [], error: null }));
  expect(view.result.current.canEdit).toBe(false);
});

test.each(['readonly', 'paused', 'reconnect'])('never offers editing for a %s saved grant', async kind => {
  const source: ProviderSource = { sourceId: 'public-provider', readerSourceId: reference.sourceId, name: 'Public notes', kind: 'google_drive',
    enabled: kind !== 'paused', health: kind === 'reconnect' ? 'reconnect_required' : 'ready', selectedScope: { kind: 'drive_folder', folderId: 'root', recursive: true },
    capabilities: { read: true, list: true, changes: false, assets: false, guardedWrite: kind !== 'readonly' }, coverage: { kind: 'complete_selected', observedAtMs: 1 } };
  const state = vi.spyOn(nativeProvidersBackend, 'state').mockResolvedValue({ sources: [source], availability: [], pendingAdmissions: [], error: null });
  const view = renderHook(() => useProviderEditing(reference, 'public-tab', vi.fn()));
  await waitFor(() => expect(state).toHaveBeenCalledOnce());
  expect(view.result.current.canEdit).toBe(false);
});

function source(): ProviderSource { return { sourceId: 'public-provider', readerSourceId: reference.sourceId, name: 'Public notes', kind: 'google_drive', enabled: true,
  selectedScope: { kind: 'drive_folder', folderId: 'root', recursive: true }, capabilities: { read: true, list: true, changes: true, assets: false, guardedWrite: true }, health: 'ready', coverage: { kind: 'never_indexed' } }; }
function projection(revision = reference.revision) { return { jobId: 'public-job', readerSourceId: reference.sourceId, path: reference.path, revision }; }
function installLocalProjection(revision = reference.revision) {
  vi.spyOn(nativeProvidersBackend, 'documents').mockResolvedValue({ operation: 'documents', offset: 0, nextOffset: null, documents: [{ documentId: 'exact-public-handle', name: 'Public.md', displayPath: 'Public.md', modifiedAtMs: 1, sizeBytes: 0, admitted: true, cached: true, cachedObservedAtMs: 1, projection: projection() }] });
  return vi.spyOn(nativeProvidersBackend, 'fetchCurrent').mockResolvedValue(projection(revision));
}
function grant() { vi.spyOn(nativeProvidersBackend, 'state').mockResolvedValue({ sources: [source()], availability: [], pendingAdmissions: [], error: null }); }
function ticket(revision: string) { return { operation: 'begin_edit_exact' as const, writeSourceId: 'public-provider', editId: 'exact-edit', markdown: '# Current public note', expiresAtMs: 10000, projection: projection(revision) }; }
test('explicit Edit from an old Recent receipt fetches current head before exact begin and keeps the same tab', async () => {
  grant(); const fetch = installLocalProjection('e'.repeat(64));
  const begin = vi.spyOn(providerEditBackend, 'begin').mockResolvedValue(ticket('e'.repeat(64)));
  const notify = vi.fn();
  const view = renderHook(() => { const history = useCanonicalHistory(); return { history, editing: useProviderEditing(history.reference, history.tab?.id, notify) }; });
  act(() => view.result.current.history.setReference(reference));
  await waitFor(() => expect(view.result.current.editing.canEdit).toBe(true));
  const initialId = useReaderHistoryStore.getState().data.activeTabId;
  await act(async () => view.result.current.editing.edit(view.result.current.history.reference!));
  expect(fetch).toHaveBeenCalledExactlyOnceWith('public-provider', 'exact-public-handle');
  expect(begin).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ ...reference, revision: 'e'.repeat(64) }));
  expect(view.result.current.history.reference?.revision).toBe('e'.repeat(64));
  expect(useReaderHistoryStore.getState().data.tabs).toHaveLength(1); expect(useReaderHistoryStore.getState().data.activeTabId).toBe(initialId);
  expect(view.result.current.editing.activeKey).toBe('cloud:mdreader-provider:exact-edit'); expect(notify).not.toHaveBeenCalled();
});
test('a selection switch during current fetch prevents begin and retarget', async () => {
  grant(); const fetch = installLocalProjection(); let finish!: (value: ReturnType<typeof projection>) => void;
  fetch.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const begin = vi.spyOn(providerEditBackend, 'begin'); const notify = vi.fn();
  const view = renderHook(() => { const history = useCanonicalHistory(); return { history, editing: useProviderEditing(history.reference, history.tab?.id, notify) }; });
  act(() => view.result.current.history.setReference(reference)); await waitFor(() => expect(view.result.current.editing.canEdit).toBe(true));
  let request!: Promise<void>; act(() => { request = view.result.current.editing.edit(view.result.current.history.reference!); });
  await waitFor(() => expect(fetch).toHaveBeenCalledOnce());
  act(() => view.result.current.history.setReference({ ...reference, path: `records/${'f'.repeat(64)}.md` }));
  await act(async () => { finish(projection('e'.repeat(64))); await request; });
  expect(begin).not.toHaveBeenCalled(); expect(view.result.current.history.reference?.path).toBe(`records/${'f'.repeat(64)}.md`); expect(notify).not.toHaveBeenCalled();
});
test('dirty retained draft reopens without fetching or replacing its base receipt', async () => {
  grant(); const fetch = installLocalProjection('e'.repeat(64));
  const begin = vi.spyOn(providerEditBackend, 'begin').mockResolvedValue(ticket(reference.revision));
  const view = renderHook(() => { const history = useCanonicalHistory(); return { history, editing: useProviderEditing(history.reference, history.tab?.id, vi.fn()) }; });
  act(() => view.result.current.history.setReference(reference)); await waitFor(() => expect(view.result.current.editing.canEdit).toBe(true));
  await act(async () => view.result.current.editing.editValidated(view.result.current.history.reference!));
  const key = view.result.current.editing.activeKey!; const tabId = useProviderEdits.getState().edits[key]!.tabId;
  useEditorStore.getState().edit(tabId, key, '# Existing unsaved text');
  await act(async () => view.result.current.editing.edit(view.result.current.history.reference!));
  expect(fetch).not.toHaveBeenCalled(); expect(begin).toHaveBeenCalledOnce(); expect(view.result.current.history.reference?.revision).toBe(reference.revision);
  expect(useEditorStore.getState().entry(tabId, key)?.buffer).toBe('# Existing unsaved text');
});
