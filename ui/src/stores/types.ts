/** Shared shapes for the shell's zustand stores. */

/** A filesystem-backed document. Its path is only valid local-file authority. */
export interface LocalDocumentRef {
  kind?: 'local';
  path: string;
  name: string;
}

/** A provider-owned document. Keys and edit authority remain native-issued. */
export interface CloudDocumentRef {
  kind: 'cloud';
  /** Deliberately uninhabitable: cloud references cannot supply path authority. */
  path?: never;
  sourceId: string;
  recordKey: string;
  documentKey: string;
  name: string;
}

/** Identifies a document without confusing provider keys for filesystem paths. */
export type DocumentRef = LocalDocumentRef | CloudDocumentRef;

/** The stable, non-filesystem key used by tabs and transient editor buffers. */
export function documentKey(ref: DocumentRef): string {
  return ref.kind === 'cloud'
    ? `cloud:${ref.sourceId}:${ref.documentKey}`
    : `local:${ref.path}`;
}

/** Narrows a reference before it crosses a filesystem-only boundary. */
export function isLocalDocument(ref: DocumentRef): ref is LocalDocumentRef {
  return ref.kind !== 'cloud';
}

/** @deprecated Use {@link DocumentRef}; retained for local-only callers. */
export type DocRef = LocalDocumentRef;

/**
 * A recently-opened document, tagged with the workspace root it belongs to.
 * Recent is scoped per-workspace: an entry only surfaces while its own workspace
 * is active, so a document retired from workspace A can never be reopened under
 * workspace B's grant (which would escape the root and produce an error tab).
 */
export interface RecentDoc extends LocalDocumentRef {
  /** The granted workspace root this document was opened under (`null` if it was
   *  opened before any workspace grant existed — e.g. a legacy persisted entry). */
  root: string | null;
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
  docRef: DocumentRef;
  scroll: ScrollPos;
  /** Back/forward stack of documents visited in this tab (wikilink nav). */
  history: DocumentRef[];
  historyIndex: number;
  pinned: boolean;
  /**
   * The workspace root this tab was opened under, captured at open time. A
   * workspace switch retires the tabs whose `originRoot` differs from the new
   * root and records each under ITS OWN origin — so retirement never depends on
   * snapshotting the (transient, race-prone) current root. `null` for a tab
   * opened before any grant, or restored from a legacy session.
   */
  originRoot: string | null;
}

/** A closed tab retained for reopen (Cmd+Shift+T). */
export interface ClosedTab {
  docRef: DocumentRef;
  history: DocumentRef[];
  historyIndex: number;
  scroll: ScrollPos;
  pinned: boolean;
  /** Visual index the tab occupied, so reopen restores its place. */
  index: number;
  /** The tab's origin workspace root (restored on reopen). */
  originRoot: string | null;
}
