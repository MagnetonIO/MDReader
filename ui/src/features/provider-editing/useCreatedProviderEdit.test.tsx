import { act, renderHook } from '@testing-library/react';
import { beforeEach, expect, test, vi } from 'vitest';
import { useReaderHistoryStore } from '../reader-history/store';
import { useCanonicalHistory } from '../reader-history/useCanonicalHistory';
import { useCreatedProviderEdit } from './useCreatedProviderEdit';
import type { ProviderReaderReference } from '../../platform/providers';
const target: ProviderReaderReference = { sourceId: `provider:${'a'.repeat(64)}`, path: `records/${'b'.repeat(64)}.md`, revision: 'c'.repeat(64), title: 'Created.md', line: 1 };
beforeEach(() => useReaderHistoryStore.getState().reset());
test('published creation opens its exact receipt and edits only after the new grant observation is ready', async () => {
  const edit = vi.fn().mockResolvedValue(undefined), refresh = vi.fn();
  const view = renderHook(ready => { const history = useCanonicalHistory(); return { history, created: useCreatedProviderEdit(history.reference, ready, edit, history.setReference, refresh) }; }, { initialProps: false });
  act(() => view.result.current.created(target));
  expect(view.result.current.history.reference).toMatchObject(target);
  expect(refresh).toHaveBeenCalledOnce();
  expect(edit).not.toHaveBeenCalled();
  view.rerender(true);
  expect(edit).toHaveBeenCalledWith(view.result.current.history.reference);
  view.rerender(true);
  expect(edit).toHaveBeenCalledOnce();
});
test('selection changes cancel deferred created-document editing', () => {
  const edit = vi.fn().mockResolvedValue(undefined);
  const view = renderHook(ready => { const history = useCanonicalHistory(); return { history, created: useCreatedProviderEdit(history.reference, ready, edit, history.setReference, vi.fn()) }; }, { initialProps: false });
  act(() => view.result.current.created(target));
  act(() => view.result.current.history.setReference({ ...target, revision: 'd'.repeat(64) }));
  view.rerender(true);
  expect(edit).not.toHaveBeenCalled();
});
