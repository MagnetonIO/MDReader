import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { useEditorStore } from '../../stores/editor';
import { flushEditorDrafts } from '../draft-recovery/runtime';
import { useEditorExit } from './useEditorExit';
vi.mock('../draft-recovery/runtime', () => ({ flushEditorDrafts: vi.fn() }));
const store = () => useEditorStore.getState();
beforeEach(() => { vi.resetAllMocks(); store().reset(); store().seed('tab', '/public/note.md', '# Saved original', 'revision', 'preview'); });
afterEach(() => { cleanup(); store().reset(); });
test('Done checkpoints a dirty draft before reading, retaining the unsaved body and original revision', async () => {
  store().edit('tab', '/public/note.md', '# Unsaved');
  let settle!: () => void; vi.mocked(flushEditorDrafts).mockImplementation(() => new Promise(done => { settle = done; }));
  const read = vi.fn(); const hook = renderHook(() => useEditorExit('tab', '/public/note.md', read));
  let done!: Promise<void>; act(() => { done = hook.result.current.done(); });
  expect(hook.result.current.busy).toBe(true); expect(read).not.toHaveBeenCalled();
  await act(async () => { settle(); await done; });
  expect(read).toHaveBeenCalledOnce();
  expect(store().entry('tab', '/public/note.md')).toMatchObject({ buffer: '# Unsaved', savedContent: '# Saved original', baseHash: 'revision', dirty: true });
});
test('failed preservation keeps editing and Retry can leave after recovery succeeds', async () => {
  store().edit('tab', '/public/note.md', '# Unsaved'); vi.mocked(flushEditorDrafts).mockRejectedValueOnce(new Error('/private/path token=secret')).mockResolvedValue(undefined);
  const read = vi.fn(); const hook = renderHook(() => useEditorExit('tab', '/public/note.md', read));
  await act(async () => { await hook.result.current.done(); });
  expect(read).not.toHaveBeenCalled(); expect(hook.result.current.error).toBe('Your draft could not be kept. Retry or save a copy before leaving.');
  await act(async () => { await hook.result.current.done(); }); expect(read).toHaveBeenCalledOnce();
});
test('late preservation never exits a newly selected document', async () => {
  store().edit('tab', '/public/note.md', '# Unsaved'); let settle!: () => void;
  vi.mocked(flushEditorDrafts).mockImplementation(() => new Promise(done => { settle = done; }));
  const read = vi.fn(); const hook = renderHook(({ path }) => useEditorExit('tab', path, read), { initialProps: { path: '/public/note.md' } });
  let done!: Promise<void>; act(() => { done = hook.result.current.done(); });
  hook.rerender({ path: '/public/second.md' }); await act(async () => { settle(); await done; });
  expect(read).not.toHaveBeenCalled(); expect(store().entry('tab', '/public/note.md')?.dirty).toBe(true);
});
test('a save settling during Done preserves newer typing without issuing another save', async () => {
  store().edit('tab', '/public/note.md', '# First draft'); const save = store().beginSave('tab', '/public/note.md')!;
  store().edit('tab', '/public/note.md', '# Newer draft');
  vi.mocked(flushEditorDrafts).mockImplementationOnce(async () => { save.saved('new revision', '# First draft'); save.finish(); }).mockResolvedValue(undefined);
  const read = vi.fn(); const hook = renderHook(() => useEditorExit('tab', '/public/note.md', read));
  await act(async () => { await hook.result.current.done(); });
  expect(flushEditorDrafts).toHaveBeenCalledTimes(2); expect(read).toHaveBeenCalledOnce();
  expect(store().entry('tab', '/public/note.md')).toMatchObject({ dirty: true, buffer: '# Newer draft', savedContent: '# First draft', baseHash: 'new revision' });
});
