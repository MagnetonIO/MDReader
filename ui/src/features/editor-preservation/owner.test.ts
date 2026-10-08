import { afterEach, expect, test, vi } from 'vitest';
import { useEditorStore } from '../../stores/editor';
import { editorAdmission } from './admission';
import { createRendererEditorOwner, type RendererEditorOwner } from './owner';
import type { FlushRequest } from '../../shell/persistence';

const boundary = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: boundary.invoke }));
vi.mock('../../shell/persistence', () => ({
  verifyPreservationIdentity: (request: FlushRequest) => {
    if (request.identity.ownerEpoch !== '11111111-1111-1111-1111-111111111111') throw new Error('stale renderer');
  },
}));
const request = (id: string): FlushRequest => ({
  requestId: id,
  identity: { ownerEpoch: '11111111-1111-1111-1111-111111111111', rendererEpoch: '22222222-2222-2222-2222-222222222222' },
});
const json = request('33333333-3333-3333-3333-333333333333');
const editor = request('44444444-4444-4444-4444-444444444444');
let owner: RendererEditorOwner | undefined;
afterEach(() => {
  owner?.release(json, 'json'); owner?.release(editor, 'editor');
  owner = undefined;
  useEditorStore.getState().reset(); boundary.invoke.mockReset();
});
function seed() {
  const store = useEditorStore.getState();
  store.seed('tab', '/synthetic/a.md', 'saved', 'h0', 'source');
  store.edit('tab', '/synthetic/a.md', 'draft');
  store.seed('inactive', '/synthetic/other.md', 'other draft', 'h2', 'split');
}

test('actual save ownership drains once, archives every buffer and joins both exact native releases', async () => {
  seed();
  const lease = useEditorStore.getState().beginSave('tab', '/synthetic/a.md')!;
  const resumed = vi.fn(); owner = createRendererEditorOwner(resumed, vi.fn());
  boundary.invoke.mockImplementation(async (_command, { submission }) => ({
    ...submission.request, generation: submission.generation, sha256: 'a'.repeat(64), entryCount: submission.entries.length,
  }));
  const preparing = owner.prepareEditor(editor);
  const flushing = owner.prepareJson(json);
  useEditorStore.getState().edit('tab', '/synthetic/a.md', 'blocked');
  await Promise.resolve(); expect(boundary.invoke).not.toHaveBeenCalled();
  expect(lease.saved('real-save-result')).toBe(true);
  lease.finish(); await Promise.all([preparing, flushing]);
  const submission = boundary.invoke.mock.calls[0]![1].submission;
  expect(submission.entries).toEqual(expect.arrayContaining([
    expect.objectContaining({ entryId: 'tab /synthetic/a.md', buffer: 'draft', savedContent: 'draft', baseHash: 'real-save-result', dirty: false }),
    expect.objectContaining({ entryId: 'inactive /synthetic/other.md', buffer: 'other draft', mode: 'split' }),
  ]));
  expect(JSON.parse(submission.providerEditsJson)).toEqual([]);
  await owner.prepareEditor(editor);
  expect(boundary.invoke).toHaveBeenCalledTimes(1);
  expect(owner.release({ ...editor, identity: { ...editor.identity, rendererEpoch: json.requestId } }, 'editor')).toBe(false);
  expect(owner.release(json, 'json')).toBe(true);
  expect(editorAdmission.denied()).toBe(true);
  expect(owner.release(editor, 'editor')).toBe(true);
  expect(editorAdmission.denied()).toBe(false);
  expect(resumed).toHaveBeenCalledTimes(1);
});

test('a lost native submit reply retains drafts and admission without resubmission or timeout release', async () => {
  seed(); owner = createRendererEditorOwner(vi.fn(), vi.fn());
  boundary.invoke.mockRejectedValue(new Error('synthetic lost reply'));
  await expect(owner.prepareEditor(editor)).rejects.toThrow('synthetic lost reply');
  await expect(owner.prepareEditor(editor)).rejects.toThrow('synthetic lost reply');
  expect(boundary.invoke).toHaveBeenCalledTimes(1);
  expect(editorAdmission.denied()).toBe(true);
  useEditorStore.getState().reset();
  expect(useEditorStore.getState().entry('tab', '/synthetic/a.md')?.buffer).toBe('draft');
  expect(owner.release(editor, 'editor')).toBe(true);
  expect(editorAdmission.denied()).toBe(false);
});
