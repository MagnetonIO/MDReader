/** Transient selection epochs fence delayed callbacks independently of saved receipts. */
import { create } from 'zustand';
import {
  emptyHistory, HISTORY_LIMITS, sameReference,
  type ReaderEntry, type ReaderHistory, type ReaderPosition, type ReaderReference,
  type ReaderTab, type RestoredHistory,
} from './model';
import { parsePosition, parseReference } from './serialization';

const promote = (ids: string[], id: string) => [id, ...ids.filter((value) => value !== id)];
const grouped = (tabs: ReaderTab[]) => [
  ...tabs.filter((tab) => tab.pinned), ...tabs.filter((tab) => !tab.pinned),
];
interface State extends RestoredHistory {
  persisted: ReaderHistory;
  changes: number;
  selection: number;
  closed: ReaderTab[];
  error: string | null;
  hydrate(restored: RestoredHistory, expectedChanges: number): void;
  open(reference: ReaderReference, title: string, position?: ReaderPosition | null, heading?: string | null): void;
  select(id: string | null): void;
  close(id: string): void;
  reopen(): boolean;
  navigate(id: string, expected: ReaderReference, selection: number, entry: ReaderEntry): void;
  step(direction: number): void;
  position(id: string, expected: ReaderReference, selection: number, position: ReaderPosition): void;
  pin(id: string): void;
  reorder(from: number, to: number): void;
  removeRecent(reference: ReaderReference): void;
  reset(): void;
}
const initial = (): RestoredHistory => ({ mode: 'transient', data: emptyHistory(), diagnostic: null });
const selected = (data: ReaderHistory) => data.tabs.find((tab) => tab.id === data.activeTabId);
function valid(entry: ReaderEntry): boolean {
  const bytes = (text: string) => new TextEncoder().encode(text).length;
  return Boolean(parseReference(entry.reference)) && bytes(entry.title) <= 512
    && (entry.heading === null || bytes(entry.heading) <= 512)
    && (entry.position === null || Boolean(parsePosition(entry.position, entry.reference.revision)));
}
function recent(data: ReaderHistory, entry: ReaderEntry): ReaderEntry[] | null {
  const items = [entry, ...data.recent.filter((item) => !sameReference(item.reference, entry.reference))];
  return items.length > HISTORY_LIMITS.recent ? null : items;
}
function current(state: State, id: string, reference: ReaderReference, selection: number): ReaderTab | null {
  const tab = selected(state.data);
  return tab?.id === id && state.selection === selection
    && sameReference(tab.entries[tab.index]!.reference, reference) ? tab : null;
}
export const useReaderHistoryStore = create<State>((set, get) => ({
  ...initial(), persisted: emptyHistory(), changes: 0, selection: 0, closed: [], error: null,
  hydrate(restored, expectedChanges) {
    const state = get();
    if (state.changes === expectedChanges) {
      set({ ...restored, persisted: restored.data, selection: state.selection + 1, error: null, closed: [] });
    } else {
      // A late load cannot replace a newer explicit selection, but saves still
      // need the exact loaded baseline and its native/unsupported admission.
      set({ mode: restored.mode, persisted: restored.data, diagnostic: restored.diagnostic });
    }
  },
  open(reference, title, position = null, heading = null) {
    const state = get();
    const entry: ReaderEntry = { reference, title, heading, position };
    if (!valid(entry)) {
      set({ error: 'The exact reading receipt or position is invalid.' });
      return;
    }
    const match = state.data.tabs.find((tab) => sameReference(tab.entries[tab.index]!.reference, reference));
    const nextRecent = recent(state.data, entry);
    if (!nextRecent || !match && state.data.tabs.length >= HISTORY_LIMITS.tabs) {
      set({ error: 'Reading history is full. Close a tab or explicitly remove a recent entry before opening another copy.' });
      return;
    }
    const updated = match && (position || heading) ? {
      ...match, entries: match.entries.map((item, index) => index === match.index ? entry : item),
    } : match;
    const tab = updated ?? { id: crypto.randomUUID(), entries: [entry], index: 0, pinned: false };
    set({
      data: {
        ...state.data, activeTabId: tab.id,
        tabs: match ? state.data.tabs.map((item) => item.id === match.id ? tab : item) : [...state.data.tabs, tab],
        recent: nextRecent, mru: promote(state.data.mru, tab.id),
      },
      changes: state.changes + 1, selection: state.selection + 1, error: null,
    });
  },
  select(id) {
    const state = get();
    if (id !== null && !state.data.tabs.some((tab) => tab.id === id)) return;
    set({
      data: { ...state.data, activeTabId: id, mru: id ? promote(state.data.mru, id) : state.data.mru },
      changes: state.changes + 1, selection: state.selection + 1,
    });
  },
  close(id) {
    const state = get();
    const tab = state.data.tabs.find((item) => item.id === id);
    if (!tab) return;
    const mru = state.data.mru.filter((value) => value !== id);
    set({
      data: {
        ...state.data, tabs: state.data.tabs.filter((item) => item.id !== id), mru,
        activeTabId: state.data.activeTabId === id ? mru[0] ?? null : state.data.activeTabId,
      },
      closed: [...state.closed, tab].slice(-20), changes: state.changes + 1, selection: state.selection + 1,
    });
  },
  reopen() {
    const state = get();
    const tab = state.closed.at(-1);
    if (!tab || state.data.tabs.length >= HISTORY_LIMITS.tabs) return false;
    set({
      data: { ...state.data, tabs: grouped([...state.data.tabs, tab]), activeTabId: tab.id, mru: promote(state.data.mru, tab.id) },
      closed: state.closed.slice(0, -1), changes: state.changes + 1, selection: state.selection + 1,
    });
    return true;
  },
  navigate(id, expected, selection, entry) {
    const state = get();
    const tab = current(state, id, expected, selection);
    if (!tab || !valid(entry)) return;
    const nextRecent = recent(state.data, entry);
    if (!nextRecent || tab.index + 1 >= HISTORY_LIMITS.entries) {
      set({ error: 'This tab’s reading history is full. Open the target in another tab.' });
      return;
    }
    const updated = { ...tab, entries: [...tab.entries.slice(0, tab.index + 1), entry], index: tab.index + 1 };
    set({
      data: { ...state.data, tabs: state.data.tabs.map((item) => item.id === id ? updated : item), recent: nextRecent },
      changes: state.changes + 1, selection: state.selection + 1, error: null,
    });
  },
  step(direction) {
    const state = get();
    const tab = selected(state.data);
    if (!tab) return;
    const index = tab.index + direction;
    if (index < 0 || index >= tab.entries.length) return;
    set({
      data: { ...state.data, tabs: state.data.tabs.map((item) => item.id === tab.id ? { ...item, index } : item) },
      changes: state.changes + 1, selection: state.selection + 1,
    });
  },
  position(id, expected, selection, position) {
    const state = get();
    const tab = current(state, id, expected, selection);
    if (!tab || !parsePosition(position, expected.revision)) return;
    if (JSON.stringify(tab.entries[tab.index]!.position) === JSON.stringify(position)) return;
    const entries = tab.entries.map((entry, index) => index === tab.index ? { ...entry, position } : entry);
    set({
      data: {
        ...state.data, tabs: state.data.tabs.map((item) => item.id === id ? { ...item, entries } : item),
        recent: state.data.recent.map((entry) => sameReference(entry.reference, expected) ? { ...entry, position } : entry),
      },
      changes: state.changes + 1,
    });
  },
  pin(id) {
    const state = get();
    set({
      data: { ...state.data, tabs: grouped(state.data.tabs.map((tab) => tab.id === id ? { ...tab, pinned: !tab.pinned } : tab)) },
      changes: state.changes + 1,
    });
  },
  reorder(from, to) {
    const state = get();
    const tabs = [...state.data.tabs];
    const tab = tabs[from];
    if (!tab || !tabs[to] || tab.pinned !== tabs[to]!.pinned) return;
    tabs.splice(from, 1);
    tabs.splice(to, 0, tab);
    set({ data: { ...state.data, tabs }, changes: state.changes + 1 });
  },
  removeRecent(reference) {
    const state = get();
    set({
      data: { ...state.data, recent: state.data.recent.filter((entry) => !sameReference(entry.reference, reference)) },
      changes: state.changes + 1, error: null,
    });
  },
  reset() { set({ ...initial(), persisted: emptyHistory(), changes: 0, selection: 0, closed: [], error: null }); },
}));
