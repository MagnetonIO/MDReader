import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { useCloudRefresh } from './useCloudRefresh';
import type { CatalogSource } from './backend';

afterEach(() => { cleanup(); vi.useRealTimers(); });
const source: CatalogSource = { catalogId: 'remote:selected', name: 'Notes', kind: 'dropbox', enabled: true,
  librarySourceId: null, failure: null, coverage: 'selected_metadata', observedAtMs: 1 };

test('open/focus/online checks coalesce, throttle automatically, and manual refresh bypasses the minute', async () => {
  vi.useFakeTimers(); vi.setSystemTime(100_000);
  let finish!: () => void;
  const refresh = vi.fn(() => new Promise<void>(resolve => { finish = resolve; }));
  const view = renderHook(() => useCloudRefresh({ source, scopeKey: 'library1', refresh }));
  await act(async () => {});
  expect(refresh).toHaveBeenCalledTimes(1);
  await act(async () => { window.dispatchEvent(new Event('focus')); window.dispatchEvent(new Event('online')); });
  expect(refresh).toHaveBeenCalledTimes(1);
  await act(async () => finish());
  await act(async () => window.dispatchEvent(new Event('online')));
  expect(refresh).toHaveBeenCalledTimes(1);
  await act(async () => { void view.result.current(source, true); });
  expect(refresh).toHaveBeenCalledTimes(2);
  await act(async () => finish());
  await act(async () => vi.advanceTimersByTimeAsync(60_000));
  expect(refresh).toHaveBeenCalledTimes(3);
  await act(async () => finish());
});

test('a transient failed observation recovers online; authorization failure requires explicit reconnect', async () => {
  const refresh = vi.fn(async () => {});
  const props = { source: { ...source, failure: 'unavailable' }, scopeKey: 'one', refresh };
  const view = renderHook(value => useCloudRefresh(value), { initialProps: props });
  await act(async () => {});
  expect(refresh).toHaveBeenCalledWith(props.source);
  view.rerender({ ...props, source: { ...source, failure: 'unauthorized' }, scopeKey: 'two' });
  await act(async () => { window.dispatchEvent(new Event('online')); await view.result.current({ ...source, failure: 'unauthorized' }, true); });
  expect(refresh).toHaveBeenCalledTimes(1);
});

test('retiring the scope before dispatch drops its automatic request and admits only the current source', async () => {
  const refresh = vi.fn(async () => {});
  const view = renderHook(value => useCloudRefresh(value), { initialProps: { source, scopeKey: 'one', refresh } });
  view.rerender({ source: { ...source, catalogId: 'remote:new' }, scopeKey: 'two', refresh });
  await act(async () => {});
  expect(refresh).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ catalogId: 'remote:new' }));
});
