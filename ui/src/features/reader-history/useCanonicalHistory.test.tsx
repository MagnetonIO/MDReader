import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, expect, test } from 'vitest';
import { useCanonicalHistory } from './useCanonicalHistory';
import { useReaderHistoryStore } from './store';
import { receipt } from './references';
afterEach(() => { cleanup(); useReaderHistoryStore.getState().reset(); });
const origin = { sourceId: 'a'.repeat(64), path: 'origin.md', revision: 'b'.repeat(64), title: 'Origin' };
const linked = { ...origin, path: 'linked.md', revision: 'c'.repeat(64), title: 'Linked', heading: 'section-two' };
test('explicit fragment beats prior state only for its successful selection; position writes stay stable and Back/Forward resume', () => {
  const { result } = renderHook(useCanonicalHistory);
  act(() => result.current.setReference(origin));
  act(() => result.current.savePosition({ revision: origin.revision, line: 3, block: 1, offset: 20 }));
  act(() => result.current.navigate(linked));
  expect(result.current.reference?.heading).toBe('section-two');
  const reference = result.current.reference;
  act(() => result.current.savePosition({ revision: linked.revision, line: 9, block: 3, offset: 80 }));
  expect(result.current.reference).toBe(reference);
  act(() => result.current.back());
  expect(result.current.reference?.path).toBe(origin.path);
  expect(result.current.entry?.position?.offset).toBe(20);
  act(() => result.current.forward());
  expect(result.current.reference?.heading).toBeUndefined();
  expect(result.current.entry?.position?.offset).toBe(80);
});
test('a restored entry with a saved position resumes, while reopening with an explicit fragment honors it', () => {
  useReaderHistoryStore.getState().open(receipt(linked), linked.title,
    { revision: linked.revision, line: 9, block: 3, offset: 80 }, linked.heading);
  const { result } = renderHook(useCanonicalHistory);
  expect(result.current.reference?.heading).toBeUndefined();
  act(() => result.current.setReference(linked));
  expect(result.current.reference?.heading).toBe('section-two');
});
test('fallback heading for a restored entry without a saved position stays stable after the first observation', () => {
  useReaderHistoryStore.getState().open(receipt(linked), linked.title, null, linked.heading);
  const { result } = renderHook(useCanonicalHistory);
  const reference = result.current.reference;
  expect(reference?.heading).toBe('section-two');
  act(() => result.current.savePosition({ revision: linked.revision, line: 9, block: 3, offset: 80 }));
  expect(result.current.reference).toBe(reference);
});
test('reopen intent is transient; link, Back and explicit creation selections remain exact', () => {
  const { result } = renderHook(useCanonicalHistory);
  act(() => result.current.setReference(origin));
  expect(useReaderHistoryStore.getState().selectionIntent).toBe('reopen');
  expect('selectionIntent' in useReaderHistoryStore.getState().data).toBe(false);
  act(() => result.current.navigate(linked)); expect(useReaderHistoryStore.getState().selectionIntent).toBe('exact');
  act(() => result.current.back()); expect(useReaderHistoryStore.getState().selectionIntent).toBe('exact');
  act(() => result.current.setReference(linked, 'exact')); expect(useReaderHistoryStore.getState().selectionIntent).toBe('exact');
  act(() => useReaderHistoryStore.getState().select(result.current.tab!.id)); expect(useReaderHistoryStore.getState().selectionIntent).toBe('reopen');
});
