import { beforeEach, describe, expect, test } from 'vitest';

import { canGoBack, canGoForward, selectActiveTab, useTabsStore } from './tabs.js';
import type { DocRef } from './types.js';

const store = useTabsStore;
const ref = (name: string): DocRef => ({ path: `/ws/${name}.md`, name: `${name}.md` });

beforeEach(() => store.getState().reset());

describe('openTab', () => {
  test('opens a tab, activates it, and seeds single-entry history', () => {
    const id = store.getState().openTab(ref('a'));
    const state = store.getState();
    expect(state.tabs).toHaveLength(1);
    expect(state.activeTabId).toBe(id);
    const tab = selectActiveTab(state)!;
    expect(tab.docRef.path).toBe('/ws/a.md');
    expect(tab.history).toHaveLength(1);
    expect(tab.historyIndex).toBe(0);
    expect(tab.pinned).toBe(false);
  });

  test('focuses the existing tab instead of duplicating', () => {
    const first = store.getState().openTab(ref('a'));
    store.getState().openTab(ref('b'));
    const again = store.getState().openTab(ref('a'));
    expect(again).toBe(first);
    expect(store.getState().tabs).toHaveLength(2);
    expect(store.getState().activeTabId).toBe(first);
  });

  test('inserts a new tab immediately after the active one', () => {
    store.getState().openTab(ref('a'));
    const b = store.getState().openTab(ref('b'));
    store.getState().setActiveTab(store.getState().tabs[0]!.id);
    store.getState().openTab(ref('c'));
    const paths = store.getState().tabs.map((tab) => tab.docRef.name);
    expect(paths).toEqual(['a.md', 'c.md', 'b.md']);
    expect(b).toBeTruthy();
  });
});

describe('history', () => {
  test('navigateInTab pushes forward history and updates docRef', () => {
    const id = store.getState().openTab(ref('a'));
    store.getState().navigateInTab(id, ref('b'));
    const tab = selectActiveTab(store.getState())!;
    expect(tab.docRef.name).toBe('b.md');
    expect(tab.history.map((h) => h.name)).toEqual(['a.md', 'b.md']);
    expect(canGoBack(tab)).toBe(true);
    expect(canGoForward(tab)).toBe(false);
  });

  test('back then forward walks the history', () => {
    const id = store.getState().openTab(ref('a'));
    store.getState().navigateInTab(id, ref('b'));
    store.getState().navigateInTab(id, ref('c'));
    store.getState().back(id);
    expect(selectActiveTab(store.getState())!.docRef.name).toBe('b.md');
    store.getState().forward(id);
    expect(selectActiveTab(store.getState())!.docRef.name).toBe('c.md');
  });

  test('navigating after going back truncates the forward branch', () => {
    const id = store.getState().openTab(ref('a'));
    store.getState().navigateInTab(id, ref('b'));
    store.getState().back(id);
    store.getState().navigateInTab(id, ref('c'));
    const tab = selectActiveTab(store.getState())!;
    expect(tab.history.map((h) => h.name)).toEqual(['a.md', 'c.md']);
    expect(canGoForward(tab)).toBe(false);
  });

  test('navigating to the same doc is a no-op', () => {
    const id = store.getState().openTab(ref('a'));
    store.getState().navigateInTab(id, ref('a'));
    expect(selectActiveTab(store.getState())!.history).toHaveLength(1);
  });
});

describe('close and reopen', () => {
  test('closing the active tab activates the next most-recent', () => {
    const a = store.getState().openTab(ref('a'));
    const b = store.getState().openTab(ref('b'));
    store.getState().setActiveTab(a);
    store.getState().setActiveTab(b);
    store.getState().closeTab(b);
    expect(store.getState().activeTabId).toBe(a);
    expect(store.getState().tabs).toHaveLength(1);
  });

  test('reopen restores the closed tab at its old index and content', () => {
    store.getState().openTab(ref('a'));
    const b = store.getState().openTab(ref('b'));
    store.getState().openTab(ref('c'));
    store.getState().closeTab(b);
    const reopened = store.getState().reopenClosedTab();
    expect(reopened).not.toBeNull();
    const names = store.getState().tabs.map((tab) => tab.docRef.name);
    expect(names).toEqual(['a.md', 'b.md', 'c.md']);
    expect(store.getState().activeTabId).toBe(reopened);
  });

  test('reopen preserves history depth', () => {
    const a = store.getState().openTab(ref('a'));
    store.getState().navigateInTab(a, ref('b'));
    store.getState().closeTab(a);
    const reopened = store.getState().reopenClosedTab()!;
    const tab = store.getState().tabs.find((t) => t.id === reopened)!;
    expect(tab.history.map((h) => h.name)).toEqual(['a.md', 'b.md']);
    expect(tab.docRef.name).toBe('b.md');
  });

  test('reopen with an empty stack is a no-op returning null', () => {
    expect(store.getState().reopenClosedTab()).toBeNull();
  });
});

describe('MRU cycle', () => {
  test('cycle walks a frozen snapshot then commits on end', () => {
    const a = store.getState().openTab(ref('a'));
    const b = store.getState().openTab(ref('b'));
    const c = store.getState().openTab(ref('c'));
    // mru is [c, b, a] (c active/most-recent).
    store.getState().cycleMru(1);
    expect(store.getState().activeTabId).toBe(b);
    store.getState().cycleMru(1);
    expect(store.getState().activeTabId).toBe(a);
    store.getState().cycleMru(1);
    expect(store.getState().activeTabId).toBe(c);
    store.getState().endMruCycle();
    expect(store.getState().cycling).toBe(false);
    // committing c keeps it front; the run order proves a/b/c all reachable.
    expect(store.getState().mru[0]).toBe(c);
    expect([a, b].every((id) => store.getState().mru.includes(id))).toBe(true);
  });

  test('reverse cycle lands on the least-recent tab first', () => {
    store.getState().openTab(ref('a'));
    const b = store.getState().openTab(ref('b'));
    store.getState().openTab(ref('c'));
    store.getState().cycleMru(-1); // from pos 0, -1 wraps to last of [c,b,a] => a
    expect(selectActiveTab(store.getState())!.docRef.name).toBe('a.md');
    expect(b).toBeTruthy();
  });
});

describe('pin and reorder', () => {
  test('pinning regroups pinned tabs to the left', () => {
    store.getState().openTab(ref('a'));
    store.getState().openTab(ref('b'));
    const c = store.getState().openTab(ref('c'));
    store.getState().togglePin(c);
    expect(store.getState().tabs.map((t) => t.docRef.name)).toEqual(['c.md', 'a.md', 'b.md']);
    expect(store.getState().tabs[0]!.pinned).toBe(true);
  });

  test('reorderTabs moves a tab by index', () => {
    store.getState().openTab(ref('a'));
    store.getState().openTab(ref('b'));
    store.getState().openTab(ref('c'));
    store.getState().reorderTabs(0, 2);
    expect(store.getState().tabs.map((t) => t.docRef.name)).toEqual(['b.md', 'c.md', 'a.md']);
  });

  test('setScroll records per-tab scroll position', () => {
    const a = store.getState().openTab(ref('a'));
    store.getState().setScroll(a, { block: 7, offset: 42 });
    expect(selectActiveTab(store.getState())!.scroll).toEqual({ block: 7, offset: 42 });
  });
});

describe('pin + reorder compose', () => {
  test('reorder within the pinned group preserves left grouping', () => {
    const a = store.getState().openTab(ref('a'));
    const b = store.getState().openTab(ref('b'));
    store.getState().openTab(ref('c'));
    store.getState().togglePin(a); // [a*, b, c]
    store.getState().togglePin(b); // [a*, b*, c]
    // Pinned group occupies indices 0..1; move a(0) after b.
    store.getState().reorderTabs(0, 1);
    expect(store.getState().tabs.map((t) => t.docRef.name)).toEqual(['b.md', 'a.md', 'c.md']);
    expect(store.getState().tabs.map((t) => t.pinned)).toEqual([true, true, false]);
  });

  test('dragging an unpinned tab across the boundary is clamped into its group', () => {
    const a = store.getState().openTab(ref('a'));
    store.getState().openTab(ref('b'));
    store.getState().openTab(ref('c'));
    store.getState().togglePin(a); // [a*, b, c]; unpinned occupies 1..2
    // Try to drag c (2) into the pinned region (0) — clamps to index 1.
    store.getState().reorderTabs(2, 0);
    expect(store.getState().tabs.map((t) => t.docRef.name)).toEqual(['a.md', 'c.md', 'b.md']);
    expect(store.getState().tabs[0]!.pinned).toBe(true);
    expect(store.getState().tabs.slice(1).every((t) => !t.pinned)).toBe(true);
  });

  test('dragging a pinned tab across the boundary is clamped into its group', () => {
    const a = store.getState().openTab(ref('a'));
    const b = store.getState().openTab(ref('b'));
    store.getState().openTab(ref('c'));
    store.getState().togglePin(a); // [a*, b, c]
    store.getState().togglePin(b); // [a*, b*, c]; pinned occupies 0..1
    // Try to drag a (0) into the unpinned region (2) — clamps to index 1.
    store.getState().reorderTabs(0, 2);
    expect(store.getState().tabs.map((t) => t.docRef.name)).toEqual(['b.md', 'a.md', 'c.md']);
    expect(store.getState().tabs.map((t) => t.pinned)).toEqual([true, true, false]);
  });
});

describe('closeAll (workspace switch)', () => {
  test('clears every tab and returns their docRefs in tab order', () => {
    store.getState().openTab(ref('a'));
    store.getState().openTab(ref('b'));
    const retired = store.getState().closeAll();
    expect(retired.map((r) => r.name)).toEqual(['a.md', 'b.md']);
    expect(store.getState().tabs).toHaveLength(0);
    expect(store.getState().activeTabId).toBeNull();
    expect(store.getState().mru).toEqual([]);
  });

  test('clears the reopen stack — retired tabs belong to the old workspace', () => {
    store.getState().openTab(ref('a'));
    store.getState().closeAll();
    expect(store.getState().reopenClosedTab()).toBeNull();
  });
});
