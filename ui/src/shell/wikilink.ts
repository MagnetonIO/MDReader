/**
 * The wikilink-click decision, factored out of the ReaderPane so the
 * resolved / ambiguous / unresolved branching is unit-testable without a DOM.
 * Resolution runs through the engine index (see `resolveWikilink` in the fs
 * platform layer); this module only routes the outcome to the right handler.
 * A resolver that throws (no backend, engine not ready) is treated as an
 * unresolved miss so the caller can fall back to a local file-list lookup.
 */

import type { WikiResolution } from '../platform/fs.js';

export interface WikiTarget {
  path: string;
  name: string;
}

export interface NavigateWikiLinkOptions {
  /** Resolve the target against the index (may throw when unavailable). */
  resolve: () => Promise<WikiResolution>;
  /** Exactly one match — open it, optionally at `line` (a heading's source line). */
  onResolved: (target: WikiTarget, line: number | null) => void;
  /** Several matches — surface the candidate set. */
  onAmbiguous: (candidates: WikiTarget[]) => void;
  /** No index match (or the resolver failed) — the caller falls back / toasts. */
  onUnresolved: () => void;
}

/** Route a wikilink click to the correct handler based on index resolution. */
export async function navigateWikiLink(options: NavigateWikiLinkOptions): Promise<void> {
  let resolution: WikiResolution;
  try {
    resolution = await options.resolve();
  } catch {
    options.onUnresolved();
    return;
  }

  if (resolution.status === 'resolved') {
    options.onResolved({ path: resolution.path, name: resolution.name }, resolution.line);
    return;
  }
  if (resolution.status === 'ambiguous') {
    options.onAmbiguous(resolution.candidates);
    return;
  }
  options.onUnresolved();
}
