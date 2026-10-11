/**
 * Per-(tab, document) light-editing state — the source/split buffer, its dirty
 * flag, and the conflict banner's state machine. Deliberately kept OUT of the
 * tabs store (and so out of the persisted session): a live editor buffer is
 * transient working state, never something to restore across launches.
 *
 * Keyed by BOTH the tab id and the document path. That composite key is what
 * makes "never silently drop an unsaved edit" hold across in-tab navigation: a
 * tab that navigates A→B keeps its `(tab, A)` entry untouched, so navigating back
 * to A restores the buffer, while B gets its own independent entry — and a save
 * can only ever reach the CURRENT document's entry, so it can never write one
 * doc's buffer into another (the wrong-doc guard is structural, not a check).
 *
 * The invariant the whole feature rests on — "never silently rewrite files" — is
 * encoded as the `baseHash` anchor: every save carries the hash the buffer was
 * opened/last-saved against, and the backend refuses a write whose anchor no
 * longer matches disk. `baseHash` doubling as the last self-write hash is what
 * lets the watcher recognise our own echo (see `classifyExternalChange`).
 */

import { create } from 'zustand';
import { editorAdmission, type EditorSave } from '../features/editor-preservation/admission';

/** The reading pane's mode for a (tab, doc). `read` is the default (no entry). */
export type EditorMode = 'read' | 'source' | 'preview' | 'split';

/** Why a conflict banner is up: an external change the watcher caught, or a save
 *  that hit the on-disk guard. Drives the banner copy. */
export type ConflictSource = 'external' | 'save';

/** The conflict banner's two-step state machine. */
export interface ConflictState {
  currentHash: string;
  source: ConflictSource;
  phase: 'choose' | 'confirm';
}

/** One (tab, document)'s editing state. */
export interface EditorEntry {
  /** The tab this buffer belongs to. */
  tabId: string;
  /** The document path this buffer edits (a save writes here, and only here). */
  path: string;
  mode: EditorMode;
  /** The live editor text (what CodeMirror shows and what a save sends). */
  buffer: string;
  /** The text last known to be on disk — a clean buffer equals this. */
  savedContent: string;
  /** blake3 of `savedContent`: the guard anchor the next save carries. */
  baseHash: string;
  /** `buffer !== savedContent`. */
  dirty: boolean;
  /** A pending conflict, or null. */
  conflict: ConflictState | null;
}

/** The composite key for a (tab, document). A space can't occur in a tab id. */
function keyOf(tabId: string, path: string): string {
  return `${tabId} ${path}`;
}

function makeEntry(
  tabId: string,
  path: string,
  mode: EditorMode,
  content: string,
  hash: string,
): EditorEntry {
  return {
    tabId,
    path,
    mode,
    buffer: content,
    savedContent: content,
    baseHash: hash,
    dirty: false,
    conflict: null,
  };
}

export interface EditorStore {
  entries: Record<string, EditorEntry>;
  /** Captured ownership of one actual pending save; null means admission denied. */
  beginSave: (tabId: string, path: string) => EditorSave | null;

  /** The (tab, doc) entry, or undefined when it is in read mode. */
  entry: (tabId: string, path: string) => EditorEntry | undefined;
  /** The (tab, doc) view mode (`read` when there is no entry). */
  mode: (tabId: string, path: string) => EditorMode;
  /** Whether THIS (tab, doc) has unsaved edits. */
  isDirty: (tabId: string, path: string) => boolean;
  /** Whether the tab has ANY unsaved edit (across every doc it has edited). */
  tabIsDirty: (tabId: string) => boolean;
  /** Ids of every tab with at least one unsaved edit (for the close/quit guard). */
  dirtyTabIds: () => string[];
  /** Every dirty entry (for save-all on quit / workspace switch). */
  allDirtyEntries: () => EditorEntry[];

  /**
   * Seed a (tab, doc) entry from a fresh disk read (content + its hash), entering
   * `mode`. A clean starting point. Reused as "reload from disk" — it discards any
   * local edits for THIS (tab, doc) only.
   */
  seed: (tabId: string, path: string, content: string, hash: string, mode: EditorMode) => void;
  /** Switch an existing (tab, doc) entry's mode (no-op when there is none yet). */
  setMode: (tabId: string, path: string, mode: EditorMode) => void;
  /** Record a keystroke-level edit; recomputes `dirty` against `savedContent`. */
  edit: (tabId: string, path: string, buffer: string) => void;
  /**
   * A save of `savedContent` landed: adopt the fresh hash and clear any conflict.
   * `dirty` is recomputed against the CURRENT buffer — NOT forced false — so
   * keystrokes that arrived while the save was in flight leave the tab dirty.
   */
  markSaved: (tabId: string, path: string, savedContent: string, newHash: string) => void;
  raiseConflict: (tabId: string, path: string, currentHash: string, source: ConflictSource) => void;
  toConfirmOverwrite: (tabId: string, path: string) => void;
  clearConflict: (tabId: string, path: string) => void;
  /** Drop EVERY entry for a tab (called when the tab is truly closed). */
  removeTab: (tabId: string) => void;
  /**
   * Keep only the entries `keep` returns true for; drop the rest. Used to bound
   * memory: a clean buffer for a doc a tab has navigated away from (not dirty, not
   * the tab's active doc) is evicted, while dirty buffers are always retained so
   * an unsaved edit is never lost.
   */
  retain: (keep: (tabId: string, path: string, dirty: boolean) => boolean) => void;
  reset: () => void;
}

function patch(
  set: (fn: (state: EditorStore) => Partial<EditorStore>) => void,
  tabId: string,
  path: string,
  update: (entry: EditorEntry) => EditorEntry,
): void {
  set((state) => {
    const key = keyOf(tabId, path);
    const existing = state.entries[key];
    if (!existing) return {};
    return { entries: { ...state.entries, [key]: update(existing) } };
  });
}

export const useEditorStore = create<EditorStore>((set, get) => {
  function commit(update: (state: EditorStore) => Partial<EditorStore>, replaced?: string) {
    const state = get();
    const patch = update(state);
    // Publish the ownership stamp before synchronous store subscribers observe
    // the entry, so a reentrant save cannot capture its preceding incarnation.
    editorAdmission.changed(state.entries, patch.entries ?? state.entries, replaced);
    set(patch);
  }
  function mutate(update: (state: EditorStore) => Partial<EditorStore>, replaced?: string) {
    if (!editorAdmission.denied()) commit(update, replaced);
  }
  return ({
    entries: {},
    beginSave(tabId, path) {
      const key = keyOf(tabId, path);
      const entry = get().entries[key];
      if (!entry) return null;
      return editorAdmission.save(key, entry, () => get().entries[key], (entry) => {
        commit((state) => ({ entries: { ...state.entries, [key]: entry } }));
      });
    },

    entry: (tabId, path) => get().entries[keyOf(tabId, path)],
    mode: (tabId, path) => get().entries[keyOf(tabId, path)]?.mode ?? 'read',
    isDirty: (tabId, path) => get().entries[keyOf(tabId, path)]?.dirty ?? false,
    tabIsDirty: (tabId) =>
      Object.values(get().entries).some((entry) => entry.tabId === tabId && entry.dirty),
    dirtyTabIds: () => [
      ...new Set(
        Object.values(get().entries)
          .filter((entry) => entry.dirty)
          .map((entry) => entry.tabId),
      ),
    ],
    allDirtyEntries: () => Object.values(get().entries).filter((entry) => entry.dirty),

    seed: (tabId, path, content, hash, mode) =>
      mutate((state) => ({
        entries: {
          ...state.entries,
          [keyOf(tabId, path)]: makeEntry(tabId, path, mode, content, hash),
        },
      }), keyOf(tabId, path)),

    setMode: (tabId, path, mode) => patch(mutate, tabId, path, (entry) => ({ ...entry, mode })),

    edit: (tabId, path, buffer) =>
      patch(mutate, tabId, path, (entry) => ({
        ...entry,
        buffer,
        dirty: buffer !== entry.savedContent,
      })),

    markSaved: (tabId, path, savedContent, newHash) =>
      patch(mutate, tabId, path, (entry) => ({
        ...entry,
        savedContent,
        baseHash: newHash,
        // Recompute against the LIVE buffer: typing during the save keeps it dirty.
        dirty: entry.buffer !== savedContent,
        conflict: null,
      })),

    raiseConflict: (tabId, path, currentHash, source) =>
      patch(mutate, tabId, path, (entry) => ({
        ...entry,
        conflict: { currentHash, source, phase: 'choose' },
      })),

    toConfirmOverwrite: (tabId, path) =>
      patch(mutate, tabId, path, (entry) =>
        entry.conflict ? { ...entry, conflict: { ...entry.conflict, phase: 'confirm' } } : entry,
      ),

    clearConflict: (tabId, path) =>
      patch(mutate, tabId, path, (entry) => ({ ...entry, conflict: null })),

    removeTab: (tabId) =>
      mutate((state) => {
        const next: Record<string, EditorEntry> = {};
        let changed = false;
        for (const [key, entry] of Object.entries(state.entries)) {
          if (entry.tabId === tabId) changed = true;
          else next[key] = entry;
        }
        return changed ? { entries: next } : {};
      }),

    retain: (keep) =>
      mutate((state) => {
        const next: Record<string, EditorEntry> = {};
        let changed = false;
        for (const [key, entry] of Object.entries(state.entries)) {
          if (keep(entry.tabId, entry.path, entry.dirty)) next[key] = entry;
          else changed = true;
        }
        return changed ? { entries: next } : {};
      }),

    reset: () => mutate(() => ({ entries: {} })),
  });
});
