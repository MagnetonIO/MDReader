/**
 * Per-tab light-editing state — the source/split buffer, its dirty flag, and the
 * conflict banner's state machine. Deliberately kept OUT of the tabs store (and
 * so out of the persisted session): a live editor buffer is transient working
 * state, never something to restore across launches. Keyed by tab id so each
 * tab remembers its own view mode and buffer while it is open; the entry is
 * dropped when the tab closes.
 *
 * The invariant the whole feature rests on — "never silently rewrite files" — is
 * encoded here as the `baseHash` anchor: every save carries the hash the buffer
 * was opened/last-saved against, and the backend refuses a write whose anchor no
 * longer matches disk. `baseHash` doubling as the last self-write hash is what
 * lets the watcher recognise our own echo (see `classifyExternalChange`).
 */

import { create } from 'zustand';

/** The reading pane's mode for a tab. `read` is the default (no editor entry). */
export type EditorMode = 'read' | 'source' | 'split';

/** Why a conflict banner is up: an external change the watcher caught, or a save
 *  that hit the on-disk guard. Drives the banner copy. */
export type ConflictSource = 'external' | 'save';

/** The conflict banner's two-step state machine. `choose` offers Reload / Keep
 *  mine; `confirm` is the explicit second confirmation before a force overwrite. */
export interface ConflictState {
  /** The hash the file actually holds on disk right now — the anchor a forced
   *  overwrite (or a reload) re-bases onto. */
  currentHash: string;
  /** External (watcher) vs save-time (guard) — selects the banner's message. */
  source: ConflictSource;
  /** `choose` → Reload / Keep mine; `confirm` → Overwrite / Cancel. */
  phase: 'choose' | 'confirm';
}

/** One tab's editing state. Absent from `entries` ⇒ the tab is in plain read mode. */
export interface EditorEntry {
  /** The document path this buffer was seeded for. A tab can navigate to another
   *  document (wikilink, back/forward) while an entry lingers; comparing this to
   *  the tab's current docRef.path is what prevents a save from writing this
   *  buffer to the WRONG file. */
  path: string;
  mode: EditorMode;
  /** The live editor text (what CodeMirror shows and what a save sends). */
  buffer: string;
  /** The text last known to be on disk — a clean buffer equals this. */
  savedContent: string;
  /** blake3 of `savedContent`: the guard anchor the next save carries, and the
   *  value the watcher compares a disk hash against to spot our own writes. */
  baseHash: string;
  /** `buffer !== savedContent`. */
  dirty: boolean;
  /** A pending conflict, or null when there is none. */
  conflict: ConflictState | null;
}

function makeEntry(path: string, mode: EditorMode, content: string, hash: string): EditorEntry {
  return {
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

  /** The tab's entry, or undefined when it is in read mode. */
  entry: (tabId: string) => EditorEntry | undefined;
  /** The tab's view mode (`read` when there is no entry). */
  mode: (tabId: string) => EditorMode;
  /** Whether the tab has unsaved edits. */
  isDirty: (tabId: string) => boolean;

  /**
   * Seed an entry for document `path` from a fresh disk read (content + its
   * hash), entering `mode`. A clean starting point: buffer === savedContent, no
   * conflict. Reused as the "reload from disk" action too — it discards any local
   * edits.
   */
  seed: (tabId: string, path: string, content: string, hash: string, mode: EditorMode) => void;
  /** Switch an existing entry's mode (no-op if the tab has no entry yet). */
  setMode: (tabId: string, mode: EditorMode) => void;
  /** Record a keystroke-level edit; recomputes `dirty` against `savedContent`. */
  edit: (tabId: string, buffer: string) => void;
  /**
   * A save of `savedContent` landed: adopt the fresh hash as the new anchor and
   * clear any conflict. `dirty` is recomputed against the CURRENT buffer — NOT
   * forced false — so keystrokes that arrived while the save was in flight (the
   * buffer moved past the snapshot we wrote) leave the tab dirty rather than being
   * silently marked saved. `savedContent` is exactly the bytes that reached disk.
   */
  markSaved: (tabId: string, savedContent: string, newHash: string) => void;
  /** Raise the conflict banner in its `choose` phase. Leaves the buffer intact. */
  raiseConflict: (tabId: string, currentHash: string, source: ConflictSource) => void;
  /** Advance the banner to its explicit overwrite confirmation. */
  toConfirmOverwrite: (tabId: string) => void;
  /** Dismiss the conflict banner (buffer untouched — the user keeps editing). */
  clearConflict: (tabId: string) => void;
  /** Drop a tab's entry entirely (called when the tab closes). */
  remove: (tabId: string) => void;
  reset: () => void;
}

function patch(
  set: (fn: (state: EditorStore) => Partial<EditorStore>) => void,
  tabId: string,
  update: (entry: EditorEntry) => EditorEntry,
): void {
  set((state) => {
    const existing = state.entries[tabId];
    if (!existing) return {};
    return { entries: { ...state.entries, [tabId]: update(existing) } };
  });
}

export const useEditorStore = create<EditorStore>((set, get) => ({
  entries: {},

  entry: (tabId) => get().entries[tabId],
  mode: (tabId) => get().entries[tabId]?.mode ?? 'read',
  isDirty: (tabId) => get().entries[tabId]?.dirty ?? false,

  seed: (tabId, path, content, hash, mode) =>
    set((state) => ({
      entries: { ...state.entries, [tabId]: makeEntry(path, mode, content, hash) },
    })),

  setMode: (tabId, mode) => patch(set, tabId, (entry) => ({ ...entry, mode })),

  edit: (tabId, buffer) =>
    patch(set, tabId, (entry) => ({
      ...entry,
      buffer,
      dirty: buffer !== entry.savedContent,
    })),

  markSaved: (tabId, savedContent, newHash) =>
    patch(set, tabId, (entry) => ({
      ...entry,
      savedContent,
      baseHash: newHash,
      // Recompute against the LIVE buffer: typing during the save leaves it dirty.
      dirty: entry.buffer !== savedContent,
      conflict: null,
    })),

  raiseConflict: (tabId, currentHash, source) =>
    patch(set, tabId, (entry) => ({
      ...entry,
      conflict: { currentHash, source, phase: 'choose' },
    })),

  toConfirmOverwrite: (tabId) =>
    patch(set, tabId, (entry) =>
      entry.conflict
        ? { ...entry, conflict: { ...entry.conflict, phase: 'confirm' } }
        : entry,
    ),

  clearConflict: (tabId) => patch(set, tabId, (entry) => ({ ...entry, conflict: null })),

  remove: (tabId) =>
    set((state) => {
      if (!(tabId in state.entries)) return {};
      const next = { ...state.entries };
      delete next[tabId];
      return { entries: next };
    }),

  reset: () => set({ entries: {} }),
}));
