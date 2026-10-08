import { afterEach, expect, test } from 'vitest';
import { emptyHistory, type ReaderEntry, type ReaderReference } from './model';
import { restoreReaderHistory } from './serialization';
import { useReaderHistoryStore } from './store';
import { createTabCycle } from './tab-actions';
import { useTabsStore } from '../../stores/tabs';

const local: ReaderReference = { kind: 'local', sourceId: 'a'.repeat(64), path: 'notes/a.md', revision: 'b'.repeat(64) };
const provider: ReaderReference = { kind: 'provider', sourceId: `provider:${'c'.repeat(64)}`, path: `records/${'d'.repeat(64)}.md`, revision: 'e'.repeat(64) };
const device: ReaderReference = { kind: 'device', documentId: '11111111-1111-4111-8111-111111111111', sourceId: 'f'.repeat(64), revision: '1'.repeat(64) };
const entry = (reference: ReaderReference): ReaderEntry => ({ reference, title: reference.kind, heading: null, position: null });
afterEach(() => { useReaderHistoryStore.getState().reset(); useTabsStore.getState().reset(); });

test('exact typed receipts survive a restart with revision-bound positions; unsupported rows remain read-only', () => {
  const store = useReaderHistoryStore.getState();
  store.hydrate(restoreReaderHistory(undefined, true), 0);
  store.open(local, 'Local');
  const id = useReaderHistoryStore.getState().data.activeTabId!;
  const position = { revision: local.revision, line: 17, block: 3, offset: 24 };
  store.position(id, local, useReaderHistoryStore.getState().selection, position);
  store.open(provider, 'Provider'); store.open(device, 'Device'); store.select(id);
  const saved = JSON.parse(JSON.stringify(useReaderHistoryStore.getState().data));
  store.reset(); store.hydrate(restoreReaderHistory(saved, true), 0);
  expect(useReaderHistoryStore.getState().data).toEqual(saved);
  expect(useReaderHistoryStore.getState().data.tabs[0]!.entries[0]!.position).toEqual(position);
  for (const value of [{ ...saved, version: 2 }, { ...saved, recent: [{ reference: { kind: 'future' } }] }]) {
    expect(restoreReaderHistory(value, true).mode).toBe('readonly');
  }
  expect(restoreReaderHistory(saved, false)).toMatchObject({ mode: 'transient', data: emptyHistory() });
});

test('late positions and navigation cannot mutate a replaced selection; exact revisions and close/reopen remain distinct', () => {
  const store = useReaderHistoryStore.getState();
  store.open(local, 'Local');
  const id = useReaderHistoryStore.getState().data.activeTabId!;
  store.navigate(id, local, useReaderHistoryStore.getState().selection, entry(provider));
  const oldSelection = useReaderHistoryStore.getState().selection - 1;
  const selected = useReaderHistoryStore.getState().data;
  store.position(id, local, oldSelection, { revision: local.revision, line: 8, block: 1, offset: 3 });
  store.navigate(id, local, oldSelection, entry(device));
  expect(useReaderHistoryStore.getState().data).toBe(selected);
  store.step(-1);
  store.navigate(id, local, oldSelection, entry(device));
  expect(useReaderHistoryStore.getState().data.tabs[0]!.entries).toHaveLength(2);
  expect(useReaderHistoryStore.getState().data.tabs[0]!.index).toBe(0);
  store.step(1); expect(useReaderHistoryStore.getState().data.tabs[0]!.entries[1]!.reference).toEqual(provider);
  store.open({ ...local, revision: '2'.repeat(64) }, 'New exact revision');
  const replacement = useReaderHistoryStore.getState().data.activeTabId!;
  store.position(id, provider, useReaderHistoryStore.getState().selection - 1, { revision: provider.revision, line: 4, block: 1, offset: 0 });
  expect(useReaderHistoryStore.getState().data.tabs[0]!.entries[1]!.position).toBeNull();
  store.close(replacement); expect(useReaderHistoryStore.getState().data.activeTabId).toBe(id);
  expect(store.reopen()).toBe(true);
  expect(useReaderHistoryStore.getState().data.activeTabId).toBe(replacement);
});

test('mixed MRU cycling keeps its order until Control release and never converts canonical receipts to local tabs', () => {
  useTabsStore.getState().openTab({ path: '/selected/old.md', name: 'Old' });
  const store = useReaderHistoryStore.getState(); store.open(local, 'One'); store.open(provider, 'Two');
  const ids = [...useReaderHistoryStore.getState().data.mru];
  const cycle = createTabCycle(() => store.select(null), () => {});
  cycle.cycle(1); expect(useReaderHistoryStore.getState().data.activeTabId).toBe(ids[1]);
  cycle.cycle(1); expect(useReaderHistoryStore.getState().data.activeTabId).toBeNull();
  cycle.cycle(1); expect(useReaderHistoryStore.getState().data.activeTabId).toBe(ids[0]);
  cycle.finish();
  expect(useTabsStore.getState().tabs).toHaveLength(1);
  expect(useTabsStore.getState().tabs[0]!.docRef).toEqual({ path: '/selected/old.md', name: 'Old' });
});
