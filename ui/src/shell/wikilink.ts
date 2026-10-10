/**
 * The wikilink-click decision, factored out of the ReaderPane so the
 * resolved / ambiguous / unresolved branching is unit-testable without a DOM.
 * Resolution runs through the engine index (see `resolveWikilink` in the fs
 * platform layer); this module only routes the outcome to the right handler.
 * A resolver that throws (no backend, engine not ready) is treated as an
 * unresolved miss so the caller can fall back to a local file-list lookup.
 */

import type { WikiResolution, WorkspaceFile } from '../platform/fs.js';

export interface WikiTarget {
  path: string;
  name: string;
}

/** Drop a trailing `.md`/`.markdown` extension (case-insensitive). */
function stripExtension(name: string): string {
  return name.replace(/\.(md|markdown)$/i, '');
}

/**
 * The client-side wikilink fallback: resolve `[[target]]` to a workspace file by
 * its **exact** basename (extension-insensitive, case-insensitive). Retains every
 * exact match for explicit choice after the authoritative index resolver misses,
 * deliberately exact, so
 * `[[bar]]` does NOT match `foobar.md` (a substring/`endsWith` match would open
 * the wrong note).
 */
export function findWikiTargets(
  files: WorkspaceFile[],
  target: string,
): WorkspaceFile[] {
  const wanted = stripExtension(target.trim()).toLowerCase();
  if (wanted === '') return [];
  return files.filter((file) => stripExtension(file.name).toLowerCase() === wanted);
}

/**
 * The source line of the outline heading matching `heading` (case-insensitive on
 * the heading text), or `null` when the document has no such heading. Lets the
 * client-side fallback honor a `[[Doc#heading]]` request even when the backend
 * missed and so returned no line of its own.
 */
export function headingLineFromOutline(
  outline: readonly { text: string; lineStart: number }[],
  heading: string | null | undefined,
): number | null {
  const wanted = (heading ?? '').trim().toLowerCase();
  if (wanted === '') return null;
  const item = outline.find((entry) => entry.text.trim().toLowerCase() === wanted);
  return item ? item.lineStart : null;
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
  /**
   * Checked AFTER the (async) resolution, before any handler runs: return
   * `false` to discard the navigation because the originating document or
   * workspace changed while resolving (a click whose context has moved on must
   * not navigate). Absent → always valid.
   */
  stillValid?: () => boolean;
}

/** Route a wikilink click to the correct handler based on index resolution. */
export async function navigateWikiLink(options: NavigateWikiLinkOptions): Promise<void> {
  let resolution: WikiResolution;
  try {
    resolution = await options.resolve();
  } catch {
    // A resolver failure (no backend / engine not ready) is a miss the caller
    // can fall back on — but only if the click context is still current.
    resolution = { status: 'unresolved' };
  }

  // The document/workspace the click came from changed while resolving → discard.
  if (options.stillValid && !options.stillValid()) return;

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
