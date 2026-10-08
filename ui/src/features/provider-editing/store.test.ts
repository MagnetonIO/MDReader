import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { editorAdmission } from '../editor-preservation/admission';
import { useEditorStore } from '../../stores/editor';
import { providerEditBackend } from './backend';
import { beginProviderEdit, inspectProviderEdit, saveProviderEdit, useProviderEdits } from './store';
vi.mock('./backend', () => ({ providerEditBackend: { begin: vi.fn(), prepare: vi.fn(), dispatch: vi.fn(), inspect: vi.fn(), repair: vi.fn() } }));
const reference = { sourceId: `provider:${'a'.repeat(64)}`, path: `records/${'b'.repeat(64)}.md`, revision: 'c'.repeat(64) };
const projection = { jobId: 'native-job', readerSourceId: reference.sourceId, path: reference.path, revision: 'd'.repeat(64) };
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}
async function dirty() {
  const key = (await beginProviderEdit('reader:tab', reference, () => true))!;
  useEditorStore.getState().edit('reader:tab', key, 'selected provider edit');
  return key;
}
beforeEach(() => {
  vi.resetAllMocks(); useEditorStore.getState().reset(); useProviderEdits.setState({ edits: {} });
  vi.mocked(providerEditBackend.begin).mockResolvedValue({ operation: 'begin_edit_exact', writeSourceId: 'opaque-source', editId: 'native-edit',
    markdown: '# Original', expiresAtMs: Date.now() + 60000, projection: { ...projection, revision: reference.revision } });
  vi.mocked(providerEditBackend.dispatch).mockResolvedValue(projection);
});
afterEach(() => { useEditorStore.getState().reset(); useProviderEdits.setState({ edits: {} }); });

test('provider save owns its actual buffer until dispatch settles through admission hold', async () => {
  const key = await dirty();
  const gate = deferred<typeof projection>();
  vi.mocked(providerEditBackend.dispatch).mockReturnValue(gate.promise);
  const task = saveProviderEdit(key);
  await vi.waitFor(() => expect(providerEditBackend.dispatch).toHaveBeenCalledOnce());
  const held = editorAdmission.hold()!;
  try {
    let drained = false;
    const drain = held.drain().then(() => { drained = true; });
    useEditorStore.getState().reset(); useEditorStore.getState().edit('reader:tab', key, 'denied mutation');
    await Promise.resolve(); expect(drained).toBe(false);
    expect(useEditorStore.getState().entry('reader:tab', key)?.buffer).toBe('selected provider edit');
    gate.resolve(projection); expect(await task).toBe('saved'); await drain;
    expect(useEditorStore.getState().entry('reader:tab', key)).toMatchObject({
      buffer: 'selected provider edit', savedContent: 'selected provider edit', baseHash: projection.revision, dirty: false,
    });
    expect(providerEditBackend.prepare).toHaveBeenCalledWith('opaque-source', 'native-edit', 'selected provider edit');
  } finally { held.release(); }
});
test('lost reply never resends, inspection retains a newer buffer and unavailable exact proof', async () => {
  const key = await dirty();
  vi.mocked(providerEditBackend.dispatch).mockRejectedValue(new Error('reply lost'));
  expect(await saveProviderEdit(key)).toBe('error');
  expect(useProviderEdits.getState().edits[key]?.phase).toBe('uncertain');
  expect(await saveProviderEdit(key)).toBe('error'); expect(providerEditBackend.dispatch).toHaveBeenCalledOnce();
  useEditorStore.getState().edit('reader:tab', key, 'newer buffer');
  vi.mocked(providerEditBackend.inspect).mockResolvedValue({ operation: 'inspect_save', operationId: 'native-edit', state: 'published', projection });
  await inspectProviderEdit(key);
  expect(useEditorStore.getState().entry('reader:tab', key)).toMatchObject({ buffer: 'newer buffer', savedContent: '# Original', dirty: true });
  vi.mocked(providerEditBackend.inspect).mockResolvedValue({ operation: 'inspect_save', operationId: 'native-edit', state: 'published', projection: null });
  await inspectProviderEdit(key);
  expect(useProviderEdits.getState().edits[key]?.error).toContain('exact local revision is unavailable');
  expect(useEditorStore.getState().entry('reader:tab', key)?.buffer).toBe('newer buffer'); expect(providerEditBackend.dispatch).toHaveBeenCalledOnce();
});
test('late begin cannot seed a newly selected editor', async () => {
  const gate = deferred<Awaited<ReturnType<typeof providerEditBackend.begin>>>();
  vi.mocked(providerEditBackend.begin).mockReturnValue(gate.promise);
  let selected = true;
  const task = beginProviderEdit('reader:tab', reference, () => selected); selected = false;
  gate.resolve({ operation: 'begin_edit_exact', writeSourceId: 'opaque-source', editId: 'late-edit', markdown: 'late body',
    expiresAtMs: Date.now() + 60000, projection: { ...projection, revision: reference.revision } });
  expect(await task).toBeNull(); expect(useEditorStore.getState().entries).toEqual({});
});
