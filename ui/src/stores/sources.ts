/**
 * The workspace source: a single picked root folder, its markdown file list, the
 * recently-opened documents, and the live watcher's status (surfaced in the
 * StatusBar). A single static "workspace folder" source for this phase; multiple
 * sources arrive with the engine in P3.
 */

import { create } from 'zustand';

import type { WorkspaceFile } from '../platform/fs.js';
import type { DocRef, RecentDoc } from './types.js';

export type WatcherStatus = 'idle' | 'watching' | 'error';

/** How many recently-opened documents to retain. */
export const MAX_RECENT = 12;

export interface SourcesState {
  root: string | null;
  files: WorkspaceFile[];
  recent: RecentDoc[];
  watcher: WatcherStatus;
  loading: boolean;

  setRoot: (root: string | null) => void;
  setFiles: (files: WorkspaceFile[]) => void;
  setLoading: (loading: boolean) => void;
  /** Record `docRef` as most-recent under workspace `root` (deduped by path). */
  addRecent: (docRef: DocRef, root: string | null) => void;
  setWatcher: (watcher: WatcherStatus) => void;
  hydrate: (partial: { root?: string | null; recent?: RecentDoc[] }) => void;
  reset: () => void;
}

const initialState = {
  root: null as string | null,
  files: [] as WorkspaceFile[],
  recent: [] as RecentDoc[],
  watcher: 'idle' as WatcherStatus,
  loading: false,
};

export const useSourcesStore = create<SourcesState>((set) => ({
  ...initialState,

  setRoot: (root) => set({ root }),
  setFiles: (files) => set({ files }),
  setLoading: (loading) => set({ loading }),
  addRecent: (docRef, root) =>
    set((state) => {
      const entry = { path: docRef.path, name: docRef.name, root };
      // Dedupe within the SAME workspace only — the same relative path can exist
      // in two workspaces, and each keeps its own history.
      const next = [
        entry,
        ...state.recent.filter((e) => !(e.root === root && e.path === entry.path)),
      ];
      // Cap PER workspace root (not globally, newest-first) so heavy activity in
      // one workspace can't evict another's recents.
      const perRoot = new Map<string | null, number>();
      return {
        recent: next.filter((e) => {
          const count = (perRoot.get(e.root) ?? 0) + 1;
          perRoot.set(e.root, count);
          return count <= MAX_RECENT;
        }),
      };
    }),
  setWatcher: (watcher) => set({ watcher }),
  hydrate: (partial) =>
    set((state) => ({
      root: partial.root !== undefined ? partial.root : state.root,
      recent: partial.recent ?? state.recent,
    })),
  reset: () => set({ ...initialState }),
}));
