import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { saveDraftCopy } from '../document-transfer/draft-copy';
import { useDraftCopy } from './useDraftCopy';
vi.mock('../document-transfer/draft-copy', () => ({ saveDraftCopy: vi.fn() }));
afterEach(() => { cleanup(); vi.resetAllMocks(); });
test('duplicate clicks retain one exact snapshot and cancellation never changes the draft', async () => {
  let finish!: (state: 'cancelled') => void;
  vi.mocked(saveDraftCopy).mockReturnValue(new Promise(resolve => { finish = resolve; }));
  const hook = renderHook(() => useDraftCopy('draft'));
  let pending!: Promise<void>;
  act(() => { pending = hook.result.current.save('\uFEFF# Before\r\n', 'note.md'); });
  await act(async () => hook.result.current.save('# Newer typing', 'note.md'));
  expect(saveDraftCopy).toHaveBeenCalledExactlyOnceWith('\uFEFF# Before\r\n', 'note.md');
  await act(async () => { finish('cancelled'); await pending; });
  expect(hook.result.current).toMatchObject({ busy: false, error: null, notice: 'Copy not saved. Your editor draft is kept.' });
});
test('late feedback cannot attach to another editor and a completed copy releases its button', async () => {
  let finish!: (state: 'exported') => void;
  vi.mocked(saveDraftCopy).mockReturnValue(new Promise(resolve => { finish = resolve; }));
  const hook = renderHook(({ key }) => useDraftCopy(key), { initialProps: { key: 'old-draft' } });
  let pending!: Promise<void>;
  act(() => { pending = hook.result.current.save('# Old', 'old.md'); });
  hook.rerender({ key: 'new-draft' });
  await act(async () => { finish('exported'); await pending; });
  expect(hook.result.current).toMatchObject({ busy: false, notice: null, error: null });
});
