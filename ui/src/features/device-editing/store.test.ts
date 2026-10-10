import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { useEditorStore } from '../../stores/editor';
import { editorAdmission } from '../editor-preservation/admission';
import { DeviceReaderFailure, type DeviceReaderBackend, type DeviceReference } from '../device-reader/backend';
import { beginDeviceEdit, saveDeviceEdit, useDeviceEdits } from './store';

const original: DeviceReference = { documentId: '00000000-0000-4000-8000-000000000001', sourceId: 'a'.repeat(64), revision: 'b'.repeat(64), path: 'document.md', title: 'Note.md' };
const published: DeviceReference = { ...original, documentId: '00000000-0000-4000-8000-000000000002', sourceId: 'c'.repeat(64), revision: 'd'.repeat(64) };
const backend: DeviceReaderBackend = {
  read: vi.fn(async () => ({ ...original, operation: 'read_document' as const, markdown: '# Original', lineStart: 1, lineEnd: 1 })),
  save: vi.fn(), manifest: vi.fn(async () => null), images: vi.fn(),
};
async function dirty() {
  const key = (await beginDeviceEdit('reader:tab', original, () => true, backend))!;
  useEditorStore.getState().edit('reader:tab', key, '# Edited');
  return key;
}
beforeEach(() => {
  vi.mocked(backend.save!).mockReset().mockResolvedValue(published);
  useEditorStore.getState().reset(); useDeviceEdits.setState({ edits: {} });
});
afterEach(() => { useEditorStore.getState().reset(); useDeviceEdits.setState({ edits: {} }); });

test('uncertain retry replays the exact operation and bytes while preserving newer typing', async () => {
  const key = await dirty();
  vi.mocked(backend.save!).mockRejectedValueOnce(new Error('reply lost'));
  expect(await saveDeviceEdit(key)).toBe('error');
  const first = vi.mocked(backend.save!).mock.calls[0]!;
  useEditorStore.getState().edit('reader:tab', key, '# Newer');
  expect(await saveDeviceEdit(key)).toBe('error');
  expect(vi.mocked(backend.save!).mock.calls[1]).toEqual(first);
  expect(useEditorStore.getState().entry('reader:tab', key)).toMatchObject({ buffer: '# Newer', savedContent: '# Edited', baseHash: published.revision, dirty: true });
  expect(useDeviceEdits.getState().edits[key]?.pending).toBeNull();
  expect(await saveDeviceEdit(key)).toBe('saved');
  const next = vi.mocked(backend.save!).mock.calls[2]!;
  expect(next[0]).toEqual(published); expect(next[1]).not.toBe(first[1]); expect(next[2]).toBe('# Newer');
});
test('admission hold drains the issued save and prevents a changed incarnation from becoming clean', async () => {
  const key = await dirty();
  let resolve!: (next: DeviceReference) => void;
  vi.mocked(backend.save!).mockReturnValue(new Promise((done) => { resolve = done; }));
  const task = saveDeviceEdit(key);
  useEditorStore.getState().edit('reader:tab', key, '# Newer');
  const hold = editorAdmission.hold()!;
  try {
    let drained = false;
    const drain = hold.drain().then(() => { drained = true; });
    useEditorStore.getState().edit('reader:tab', key, '# Denied');
    await Promise.resolve(); expect(drained).toBe(false);
    resolve(published); expect(await task).toBe('error'); await drain;
    expect(useEditorStore.getState().entry('reader:tab', key)).toMatchObject({ buffer: '# Newer', savedContent: '# Original', dirty: true });
  } finally { hold.release(); }
});
test('late reads cannot seed an editor after the selected document changes', async () => {
  expect(await beginDeviceEdit('reader:tab', original, () => false, backend)).toBeNull();
  expect(useEditorStore.getState().entries).toEqual({});
});
test('an adapter without an actual native save does not admit editing', async () => {
  await expect(beginDeviceEdit('reader:tab', original, () => true, { ...backend, save: undefined })).rejects.toThrow('unavailable');
  expect(useEditorStore.getState().entries).toEqual({});
});

test('a definite quota refusal preserves the draft without an unconfirmed-save claim', async () => {
  const key = await dirty();
  vi.mocked(backend.save!).mockRejectedValueOnce(new DeviceReaderFailure('quota'));
  expect(await saveDeviceEdit(key)).toBe('error');
  expect(useDeviceEdits.getState().edits[key]?.pending).toBeNull();
  expect(useDeviceEdits.getState().edits[key]?.error).toContain('saved-revision limit');
  expect(useEditorStore.getState().entry('reader:tab', key)).toMatchObject({ buffer: '# Edited', dirty: true });
});
