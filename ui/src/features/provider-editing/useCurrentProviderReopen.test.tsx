import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { useCurrentProviderReopen } from './useCurrentProviderReopen';
import { useCanonicalHistory } from '../reader-history/useCanonicalHistory';
import { useReaderHistoryStore } from '../reader-history/store';
import { useProviderEdits } from './store';
import { useEditorStore } from '../../stores/editor';
import { receipt } from '../reader-history/references';
import type { ProviderDocumentBody, ProviderReaderReference } from '../../platform/providers';
const reference = { sourceId: `provider:${'a'.repeat(64)}`, path: `records/${'b'.repeat(64)}.md`, revision: 'c'.repeat(64), title: 'Public.md' };
const body = (revision = 'd'.repeat(64)): ProviderDocumentBody => ({ ...reference, revision, operation: 'read_document', markdown: '# Public cached head', lineStart: 1, lineEnd: 1 });
const empty = () => { useReaderHistoryStore.getState().reset(); useProviderEdits.setState({ edits: {} }); useEditorStore.setState({ entries: {} }); };
afterEach(() => { cleanup(); empty(); });
function host(readCurrentDocument: (reference: ProviderReaderReference) => Promise<ProviderDocumentBody> = vi.fn(async () => body())) {
  const backend = { readCurrentDocument }, notify = vi.fn();
  const view = renderHook(() => {
    const history = useCanonicalHistory();
    useCurrentProviderReopen(history.reference, history.tab?.id, notify, backend, true);
    return history;
  });
  return { ...view, readCurrentDocument, notify };
}
test('stale Recent reopens the current admitted receipt in the same tab, retaining old placement only on the old entry', async () => {
  const prior = { revision: reference.revision, line: 41, block: 20, offset: 13 };
  useReaderHistoryStore.getState().open(receipt(reference), reference.title, prior);
  const tabId = useReaderHistoryStore.getState().data.activeTabId;
  const view = host();
  await waitFor(() => expect(view.result.current.reference?.revision).toBe('d'.repeat(64)));
  expect(view.result.current.tab?.id).toBe(tabId);
  expect(view.result.current.tab?.entries[0]?.position).toEqual(prior);
  expect(view.result.current.entry?.position).toBeNull(); expect(view.result.current.entry?.heading).toBeNull();
  expect(view.readCurrentDocument).toHaveBeenCalledOnce(); expect(view.notify).not.toHaveBeenCalled();
});
test('superseded asynchronous selection cannot retarget a newer selected document', async () => {
  let finish!: (body: ProviderDocumentBody) => void;
  const read = vi.fn((target: typeof reference) => target.path === reference.path ? new Promise<ProviderDocumentBody>(resolve => { finish = resolve; }) : Promise.resolve({ ...body(target.revision), path: target.path }));
  const view = host(read);
  act(() => view.result.current.setReference(reference));
  await waitFor(() => expect(read).toHaveBeenCalledOnce());
  const other = { ...reference, path: `records/${'e'.repeat(64)}.md`, title: 'Other.md' };
  act(() => view.result.current.setReference(other));
  await act(async () => finish(body()));
  expect(view.result.current.reference?.path).toBe(other.path); expect(view.result.current.reference?.revision).toBe(other.revision);
  expect(view.notify).not.toHaveBeenCalled();
});
function retained(phase: 'ready' | 'uncertain', dirty: boolean) {
  useReaderHistoryStore.getState().open(receipt(reference), reference.title);
  const tabId = `reader:${useReaderHistoryStore.getState().data.activeTabId}`, key = 'cloud:mdreader-provider:public-edit';
  useEditorStore.getState().seed(tabId, key, '# Public base', 'public-edit', 'source');
  if (dirty) useEditorStore.getState().edit(tabId, key, '# Kept unsaved text');
  useProviderEdits.setState({ edits: { [key]: { key, tabId, reference, phase, pendingBody: phase === 'uncertain' ? '# Retained attempt' : null, projection: null, error: null,
    ticket: { operation: 'begin_edit_exact', projection: { jobId: 'job', readerSourceId: reference.sourceId, path: reference.path, revision: reference.revision }, editId: 'public-edit', writeSourceId: 'selected', markdown: '# Public base', expiresAtMs: Date.now() + 60_000 } } } });
  return { tabId, key };
}
test.each([['ready', true], ['uncertain', false]] as const)('protected %s tab keeps its exact receipt and buffer without current-head read', async (phase, dirty) => {
  const { tabId, key } = retained(phase, dirty), edits = useProviderEdits.getState().edits, entries = useEditorStore.getState().entries;
  const view = host(); await act(async () => {});
  expect(view.readCurrentDocument).not.toHaveBeenCalled(); expect(view.result.current.reference?.revision).toBe(reference.revision);
  expect(useProviderEdits.getState().edits).toBe(edits); expect(useEditorStore.getState().entries).toBe(entries);
  expect(useEditorStore.getState().entry(tabId, key)?.buffer).toBe(dirty ? '# Kept unsaved text' : '# Public base');
});
test('a buffer becoming dirty while current-head read is pending prevents retargeting', async () => {
  let finish!: (body: ProviderDocumentBody) => void;
  useReaderHistoryStore.getState().open(receipt(reference), reference.title);
  const read = vi.fn(() => new Promise<ProviderDocumentBody>(resolve => { finish = resolve; }));
  const view = host(read); await waitFor(() => expect(read).toHaveBeenCalledOnce());
  const tabId = `reader:${view.result.current.tab!.id}`;
  useEditorStore.getState().seed(tabId, 'public-edit', '# Public base', 'edit', 'source');
  useEditorStore.getState().edit(tabId, 'public-edit', '# New unsaved text');
  await act(async () => finish(body()));
  expect(view.result.current.reference?.revision).toBe(reference.revision);
  expect(useEditorStore.getState().entry(tabId, 'public-edit')?.buffer).toBe('# New unsaved text');
});
test.each(['sourceId', 'path', 'revision'] as const)('wrong current canonical %s is rejected without changing history', async field => {
  const wrong = { ...body(), [field]: field === 'sourceId' ? `provider:${'f'.repeat(64)}` : field === 'path' ? `records/${'f'.repeat(64)}.md` : 'not-a-revision' };
  const view = host(vi.fn(async () => wrong));
  act(() => view.result.current.setReference(reference));
  await waitFor(() => expect(view.notify).toHaveBeenCalledOnce());
  expect(view.result.current.reference?.revision).toBe(reference.revision); expect(view.result.current.tab?.entries).toHaveLength(1);
});
test('internal link and Back exact selections stay strict while explicit tab reselect can reopen', async () => {
  const view = host(vi.fn(async (target: ProviderReaderReference) => ({ ...body(target.revision), path: target.path })));
  act(() => view.result.current.setReference(reference, 'exact'));
  const linked = { ...reference, path: `records/${'e'.repeat(64)}.md`, revision: 'e'.repeat(64), heading: 'section' };
  act(() => view.result.current.navigate(linked));
  await act(async () => {}); expect(view.readCurrentDocument).not.toHaveBeenCalled();
  act(() => view.result.current.back());
  await act(async () => {}); expect(view.readCurrentDocument).not.toHaveBeenCalled();
  expect(view.result.current.reference?.revision).toBe(reference.revision);
  act(() => useReaderHistoryStore.getState().select(view.result.current.tab!.id));
  await waitFor(() => expect(view.readCurrentDocument).toHaveBeenCalledOnce());
});
test('local selected receipts never contact provider current-head transport', async () => {
  const view = host();
  act(() => view.result.current.setReference({ ...reference, sourceId: 'a'.repeat(64), path: 'public.md' }));
  await act(async () => {}); expect(view.readCurrentDocument).not.toHaveBeenCalled();
});
