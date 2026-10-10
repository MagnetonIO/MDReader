import { act, renderHook } from '@testing-library/react';
import { expect, test, vi } from 'vitest';
import type { MarkdownReaderHandle } from '@mdreader/ui';
import { useReaderRestoration } from './useReaderRestoration';

function handle() {
  let settle: ((aligned: boolean) => void) | undefined;
  const value: MarkdownReaderHandle = {
    scrollToLine: vi.fn((_line, callback) => { settle = callback; }),
    scrollToFragment: vi.fn(() => false), scrollToBlock: vi.fn(), getScrollAnchor: () => null,
    cancelScrollNavigation: vi.fn(() => settle?.(false)),
  };
  return { value, settled: (aligned: boolean) => settle?.(aligned) };
}
test('a saved line cannot autosave during mounting/measurement and becomes writable only after alignment', () => {
  const reader = { current: null as MarkdownReaderHandle | null }, failed = vi.fn(), target = handle();
  const { result, rerender } = renderHook(() => useReaderRestoration(reader, failed));
  act(() => { result.current.attach(target.value, 205, null); });
  expect(target.value.scrollToLine).toHaveBeenCalledWith(205, expect.any(Function));
  expect(result.current.canSave()).toBe(false);
  // A parent rerender can replace the imperative handle object for the same
  // mounted document. It must not strand this exact pending restoration.
  rerender(); act(() => { result.current.attach({ ...target.value }, 205, null); });
  act(() => target.settled(true));
  expect(result.current.canSave()).toBe(true); expect(failed).not.toHaveBeenCalled();
});
test('failed placement preserves the saved line until real user takeover, without retrying on unrelated renders', () => {
  const reader = { current: null as MarkdownReaderHandle | null }, target = handle(), failed = vi.fn();
  const { result, rerender } = renderHook(() => useReaderRestoration(reader, failed));
  act(() => result.current.attach(target.value, 205, null));
  act(() => target.settled(false)); expect(failed).toHaveBeenCalledTimes(1); expect(result.current.canSave()).toBe(false);
  rerender(); act(() => result.current.attach(target.value, 205, null));
  expect(target.value.scrollToLine).toHaveBeenCalledTimes(1);
  act(() => result.current.userIntent()); expect(result.current.canSave()).toBe(true);
  expect(failed).toHaveBeenCalledTimes(1);
});
test('old-selection callbacks cannot enable saving or raise errors after reset; user input cancels a new pending jump', () => {
  const reader = { current: null as MarkdownReaderHandle | null }, old = handle(), next = handle(), failed = vi.fn();
  const { result } = renderHook(() => useReaderRestoration(reader, failed));
  act(() => { result.current.attach(old.value, 205, null); result.current.reset(); result.current.attach(next.value, 501, null); });
  act(() => old.settled(true)); expect(result.current.canSave()).toBe(false);
  act(() => result.current.userIntent()); expect(result.current.canSave()).toBe(true);
  expect(next.value.cancelScrollNavigation).toHaveBeenCalled(); expect(failed).not.toHaveBeenCalled();
});
test('an absent fragment falls back to its explicit line without prematurely releasing the save fence', () => {
  const reader = { current: null as MarkdownReaderHandle | null }, target = handle();
  target.value.scrollToFragment = vi.fn((_fragment, callback) => { callback?.(false); return false; });
  const { result } = renderHook(() => useReaderRestoration(reader, vi.fn()));
  expect(result.current.attach(target.value, 1, 'missing')).toBe(false);
  expect(result.current.canSave()).toBe(false);
  target.settled(true); expect(result.current.canSave()).toBe(true);
});
