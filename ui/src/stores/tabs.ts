/**
 * Tab model for the reader shell. The invariant every action preserves:
 * `tab.docRef === tab.history[tab.historyIndex]`. Only the active tab mounts a
 * Reader (enforced by the ReaderPane, not here), so this store is the single
 * source of truth for what is open, focused, pinned, and reopenable.
 *
 * MRU: `mru` is a list of tab ids, most-recent first. `activeTabId` is normally
 * `mru[0]`; the exception is a live Ctrl+Tab cycle, which walks a frozen
 * snapshot without disturbing `mru` until `endMruCycle` commits the landing tab.
 */

import { create } from 'zustand';

import type { ClosedTab, DocRef, ScrollPos, TabState } from './types.js';

const DEFAULT_SCROLL: ScrollPos = { block: 0, offset: 0 };
const MAX_CLOSED = 20;

let tabSeq = 0;
function nextTabId(): string {
  tabSeq += 1;
  return `tab-${tabSeq}`;
}

/** True when the tab can navigate back in its own history. */
export function canGoBack(tab: TabState | undefined): boolean {
  return tab !== undefined && tab.historyIndex > 0;
}

/** True when the tab can navigate forward in its own history. */
export function canGoForward(tab: TabState | undefined): boolean {
  return tab !== undefined && tab.historyIndex < tab.history.length - 1;
}

/** Move `id` to the front of the MRU list (deduped). */
function promote(mru: string[], id: string): string[] {
  return [id, ...mru.filter((existing) => existing !== id)];
}

/** Stable partition so pinned tabs stay grouped to the left, order preserved. */
function regroup(tabs: TabState[]): TabState[] {
  const pinned = tabs.filter((tab) => tab.pinned);
  const rest = tabs.filter((tab) => !tab.pinned);
  return [...pinned, ...rest];
}

export interface TabsState {
  tabs: TabState[];
  activeTabId: string | null;
  mru: string[];
  closed: ClosedTab[];
  /** Transient Ctrl+Tab cycle state. */
  cycling: boolean;
  cycleOrder: string[];
  cyclePos: number;

  /** Open (or focus, if already open) a document in its own tab. */
  openTab: (docRef: DocRef, opts?: { activate?: boolean }) => string;
  /** Replace the active document of `tabId`, pushing a history entry. */
  navigateInTab: (tabId: string, docRef: DocRef) => void;
  back: (tabId: string) => void;
  forward: (tabId: string) => void;
  closeTab: (tabId: string) => void;
  /** Reopen the most-recently-closed tab at its old position. */
  reopenClosedTab: () => string | null;
  setActiveTab: (tabId: string) => void;
  reorderTabs: (fromIndex: number, toIndex: number) => void;
  togglePin: (tabId: string) => void;
  setScroll: (tabId: string, scroll: ScrollPos) => void;
  /** Advance the Ctrl+Tab MRU cycle by `step` (typically +1 / -1). */
  cycleMru: (step: number) => void;
  /** Commit the current cycle's landing tab as most-recent. */
  endMruCycle: () => void;
  /** Restore tabs from a persisted session (active tab leads the MRU). */
  hydrate: (tabs: TabState[], activeTabId: string | null) => void;
  reset: () => void;
}

/** Advance the id sequence past any restored ids so new tabs never collide. */
function absorbIds(tabs: TabState[]): void {
  for (const tab of tabs) {
    const match = /^tab-(\d+)$/.exec(tab.id);
    if (match) tabSeq = Math.max(tabSeq, Number(match[1]));
  }
}

const initialState = {
  tabs: [] as TabState[],
  activeTabId: null as string | null,
  mru: [] as string[],
  closed: [] as ClosedTab[],
  cycling: false,
  cycleOrder: [] as string[],
  cyclePos: 0,
};

export const useTabsStore = create<TabsState>((set, get) => ({
  ...initialState,

  openTab(docRef, opts) {
    const activate = opts?.activate ?? true;
    const state = get();

    const existing = state.tabs.find((tab) => tab.docRef.path === docRef.path);
    if (existing) {
      if (activate) get().setActiveTab(existing.id);
      return existing.id;
    }

    const id = nextTabId();
    const tab: TabState = {
      id,
      docRef,
      scroll: DEFAULT_SCROLL,
      history: [docRef],
      historyIndex: 0,
      pinned: false,
    };

    const activeIndex = state.tabs.findIndex((candidate) => candidate.id === state.activeTabId);
    const insertAt = activeIndex >= 0 ? activeIndex + 1 : state.tabs.length;
    const tabs = regroup([
      ...state.tabs.slice(0, insertAt),
      tab,
      ...state.tabs.slice(insertAt),
    ]);

    set({
      tabs,
      activeTabId: activate ? id : state.activeTabId,
      mru: activate ? promote(state.mru, id) : [...state.mru, id],
      cycling: false,
    });
    return id;
  },

  navigateInTab(tabId, docRef) {
    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab;
        if (tab.docRef.path === docRef.path) return tab;
        const history = [...tab.history.slice(0, tab.historyIndex + 1), docRef];
        return {
          ...tab,
          history,
          historyIndex: history.length - 1,
          docRef,
          scroll: DEFAULT_SCROLL,
        };
      }),
    }));
  },

  back(tabId) {
    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId || !canGoBack(tab)) return tab;
        const historyIndex = tab.historyIndex - 1;
        return { ...tab, historyIndex, docRef: tab.history[historyIndex]!, scroll: DEFAULT_SCROLL };
      }),
    }));
  },

  forward(tabId) {
    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId || !canGoForward(tab)) return tab;
        const historyIndex = tab.historyIndex + 1;
        return { ...tab, historyIndex, docRef: tab.history[historyIndex]!, scroll: DEFAULT_SCROLL };
      }),
    }));
  },

  closeTab(tabId) {
    const state = get();
    const index = state.tabs.findIndex((tab) => tab.id === tabId);
    if (index < 0) return;
    const tab = state.tabs[index]!;

    const closedEntry: ClosedTab = {
      docRef: tab.docRef,
      history: tab.history,
      historyIndex: tab.historyIndex,
      scroll: tab.scroll,
      pinned: tab.pinned,
      index,
    };

    const tabs = state.tabs.filter((candidate) => candidate.id !== tabId);
    const mru = state.mru.filter((id) => id !== tabId);
    const activeTabId =
      state.activeTabId === tabId ? (mru[0] ?? null) : state.activeTabId;

    set({
      tabs,
      mru,
      activeTabId,
      closed: [...state.closed, closedEntry].slice(-MAX_CLOSED),
      cycling: false,
    });
  },

  reopenClosedTab() {
    const state = get();
    if (state.closed.length === 0) return null;
    const entry = state.closed[state.closed.length - 1]!;

    const id = nextTabId();
    const tab: TabState = {
      id,
      docRef: entry.docRef,
      scroll: entry.scroll,
      history: entry.history,
      historyIndex: entry.historyIndex,
      pinned: entry.pinned,
    };

    const insertAt = Math.min(Math.max(entry.index, 0), state.tabs.length);
    const tabs = regroup([
      ...state.tabs.slice(0, insertAt),
      tab,
      ...state.tabs.slice(insertAt),
    ]);

    set({
      tabs,
      activeTabId: id,
      mru: promote(state.mru, id),
      closed: state.closed.slice(0, -1),
      cycling: false,
    });
    return id;
  },

  setActiveTab(tabId) {
    const state = get();
    if (!state.tabs.some((tab) => tab.id === tabId)) return;
    set({
      activeTabId: tabId,
      mru: promote(state.mru, tabId),
      cycling: false,
    });
  },

  reorderTabs(fromIndex, toIndex) {
    set((state) => {
      const tabs = [...state.tabs];
      if (
        fromIndex < 0 ||
        toIndex < 0 ||
        fromIndex >= tabs.length ||
        toIndex >= tabs.length
      ) {
        return {};
      }
      const [moved] = tabs.splice(fromIndex, 1);
      tabs.splice(toIndex, 0, moved!);
      return { tabs };
    });
  },

  togglePin(tabId) {
    set((state) => {
      const toggled = state.tabs.map((tab) =>
        tab.id === tabId ? { ...tab, pinned: !tab.pinned } : tab,
      );
      return { tabs: regroup(toggled) };
    });
  },

  setScroll(tabId, scroll) {
    set((state) => ({
      tabs: state.tabs.map((tab) => (tab.id === tabId ? { ...tab, scroll } : tab)),
    }));
  },

  cycleMru(step) {
    const state = get();
    if (state.tabs.length === 0) return;

    const order = state.cycling ? state.cycleOrder : state.mru.slice();
    if (order.length === 0) return;
    const basePos = state.cycling ? state.cyclePos : 0;
    const pos = ((basePos + step) % order.length + order.length) % order.length;
    const target = order[pos]!;

    set({ cycling: true, cycleOrder: order, cyclePos: pos, activeTabId: target });
  },

  endMruCycle() {
    const state = get();
    if (!state.cycling) return;
    set({
      cycling: false,
      cycleOrder: [],
      cyclePos: 0,
      mru: state.activeTabId ? promote(state.mru, state.activeTabId) : state.mru,
    });
  },

  hydrate(tabs, activeTabId) {
    absorbIds(tabs);
    const regrouped = regroup(tabs);
    const validActive =
      activeTabId && regrouped.some((tab) => tab.id === activeTabId) ? activeTabId : null;
    const mru = validActive
      ? promote(regrouped.map((tab) => tab.id), validActive)
      : regrouped.map((tab) => tab.id);
    set({
      tabs: regrouped,
      activeTabId: validActive ?? regrouped[0]?.id ?? null,
      mru,
      closed: [],
      cycling: false,
      cycleOrder: [],
      cyclePos: 0,
    });
  },

  reset() {
    tabSeq = 0;
    set({
      tabs: [],
      activeTabId: null,
      mru: [],
      closed: [],
      cycling: false,
      cycleOrder: [],
      cyclePos: 0,
    });
  },
}));

/** Selector: the currently active tab, or undefined. */
export function selectActiveTab(state: TabsState): TabState | undefined {
  return state.tabs.find((tab) => tab.id === state.activeTabId);
}
