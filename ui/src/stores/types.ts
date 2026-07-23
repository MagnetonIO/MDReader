/** Shared shapes for the shell's zustand stores. */

/** Identifies an openable document by its absolute path plus a display name. */
export interface DocRef {
  path: string;
  name: string;
}

/** Scroll position within a reader, anchored to a top-level block. */
export interface ScrollPos {
  /** Index of the top-most (or nearest) block. */
  block: number;
  /** Pixel offset within that block. */
  offset: number;
}

/** One open tab. `docRef` always mirrors `history[historyIndex]`. */
export interface TabState {
  id: string;
  docRef: DocRef;
  scroll: ScrollPos;
  /** Back/forward stack of documents visited in this tab (wikilink nav). */
  history: DocRef[];
  historyIndex: number;
  pinned: boolean;
}

/** A closed tab retained for reopen (Cmd+Shift+T). */
export interface ClosedTab {
  docRef: DocRef;
  history: DocRef[];
  historyIndex: number;
  scroll: ScrollPos;
  pinned: boolean;
  /** Visual index the tab occupied, so reopen restores its place. */
  index: number;
}
