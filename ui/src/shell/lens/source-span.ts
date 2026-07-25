/**
 * The two coordinate translations the semantic surfaces need, both pure.
 *
 *  - **Reader selection → source lines.** The rendered document carries its
 *    provenance in the DOM: every top-level block keeps the `data-line-start` /
 *    `data-line-end` attributes the markdown pipeline stamped on it. Selecting
 *    a passage is therefore already a line range — we just have to read it back
 *    off the blocks the selection touches, rather than guess.
 *
 *  - **Located violation → an openable document.** The engine locates a
 *    violation at a *repo-relative* file; the reader opens *absolute* paths.
 *    A violation that knows its line is the entire point of the surface, so the
 *    resolution never degrades the span — it either produces a path that keeps
 *    the line range intact, or it produces nothing.
 */

/** An inclusive 1-based line range in a document's source. */
export interface LineSpan {
  startLine: number;
  endLine: number;
}

/** The attribute pair the markdown pipeline stamps on every rendered block. */
const LINE_ATTR = '[data-line-start]';

function elementOf(node: Node | null): Element | null {
  if (!node) return null;
  return node.nodeType === Node.ELEMENT_NODE ? (node as Element) : node.parentElement;
}

/** The nearest ancestor block that carries a source line range. */
function blockOf(node: Node | null, root: Element): Element | null {
  const element = elementOf(node);
  if (!element || !root.contains(element)) return null;
  return element.closest(LINE_ATTR);
}

function lineOf(block: Element, attribute: string): number | null {
  const raw = block.getAttribute(attribute);
  if (raw === null) return null;
  const value = Number(raw);
  return Number.isFinite(value) && value > 0 ? Math.trunc(value) : null;
}

/**
 * The line span a DOM range covers, or `null` when the range touches no block
 * that records one. Spans the union of the touched blocks: selecting from the
 * middle of one paragraph into the next cites both, because both contributed
 * text a claim could have been derived from.
 */
export function spanFromRange(range: Range, root: Element): LineSpan | null {
  const blocks = [blockOf(range.startContainer, root), blockOf(range.endContainer, root)];
  let start: number | null = null;
  let end: number | null = null;
  for (const block of blocks) {
    if (!block) continue;
    const blockStart = lineOf(block, 'data-line-start');
    const blockEnd = lineOf(block, 'data-line-end') ?? blockStart;
    if (blockStart !== null) start = start === null ? blockStart : Math.min(start, blockStart);
    if (blockEnd !== null) end = end === null ? blockEnd : Math.max(end, blockEnd);
  }
  if (start === null) return null;
  return { startLine: start, endLine: end ?? start };
}

/**
 * The line span of the current selection, when it is a real (non-collapsed)
 * selection inside `root`. A collapsed caret is not an inspection request, and
 * a selection made outside the reading column is not about the document — both
 * yield `null` so the panel keeps whatever passage it was already showing.
 */
export function spanFromSelection(selection: Selection | null, root: Element | null): LineSpan | null {
  if (!selection || !root || selection.isCollapsed || selection.rangeCount === 0) return null;
  const range = selection.getRangeAt(0);
  if (!root.contains(range.commonAncestorContainer)) return null;
  return spanFromRange(range, root);
}

/** Normalise Windows separators so path comparison is one shape. */
function slash(path: string): string {
  return path.replace(/\\/g, '/');
}

/**
 * Resolve a violation's repo-relative file to a path the reader can open.
 *
 * Prefers an exact match against the workspace listing (which is authoritative
 * about what exists and how it is spelled), then falls back to joining the
 * workspace root. Returns `null` when neither can produce a path, so the caller
 * disables the jump rather than opening an error tab — the violation keeps its
 * span either way, it simply is not reachable from here.
 */
export function resolveSourcePath(
  file: string,
  root: string | null,
  files: readonly { path: string }[],
): string | null {
  const wanted = slash(file).replace(/^\.?\//, '');
  if (wanted.length === 0) return null;

  const base = root === null ? null : slash(root).replace(/\/+$/, '');
  if (base !== null) {
    const joined = `${base}/${wanted}`;
    const exact = files.find((entry) => slash(entry.path) === joined);
    if (exact) return exact.path;
  }

  // No root match: accept a unique suffix match on a path segment boundary, so
  // `notes/adr-3.md` finds `/ws/notes/adr-3.md` but never `/ws/other-adr-3.md`.
  const matches = files.filter((entry) => {
    const candidate = slash(entry.path);
    return candidate === wanted || candidate.endsWith(`/${wanted}`);
  });
  if (matches.length === 1) return matches[0]!.path;

  return base === null ? null : `${base}/${wanted}`;
}
