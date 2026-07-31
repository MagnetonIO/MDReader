import type { Element, ElementContent, Root as HastRoot, RootContent } from 'hast';

/** A 1-based inclusive source line range. */
interface LineRange {
  start: number;
  end: number;
}

/** Loose view of a unist node carrying an optional position. */
interface Positioned {
  type: string;
  position?: { start: { line: number }; end: { line: number } } | null;
  children?: unknown[];
}

/** Read an element's own source range, if remark-rehype carried one onto it. */
function ownRange(node: Element): LineRange | undefined {
  const position = node.position;
  if (!position) return undefined;
  return { start: position.start.line, end: position.end.line };
}

/**
 * Derive a range for a generated container that has no `position` of its own
 * (notably the hoisted footnotes `<section>`) from the min/max source lines of
 * its positionally-anchored descendants.
 */
function deriveRange(node: ElementContent): LineRange | undefined {
  let start = Number.POSITIVE_INFINITY;
  let end = Number.NEGATIVE_INFINITY;

  const walk = (current: ElementContent): void => {
    const position = (current as Positioned).position;
    if (position) {
      if (position.start.line < start) start = position.start.line;
      if (position.end.line > end) end = position.end.line;
    }
    const children = (current as { children?: ElementContent[] }).children;
    if (Array.isArray(children)) {
      for (const child of children) walk(child);
    }
  };
  walk(node);

  if (start === Number.POSITIVE_INFINITY || end === Number.NEGATIVE_INFINITY) {
    return undefined;
  }
  return { start, end };
}

function applyStamp(node: Element, range: LineRange): void {
  node.properties = node.properties ?? {};
  node.properties.dataLineStart = String(range.start);
  node.properties.dataLineEnd = String(range.end);
}

/** True once an element already carries both provenance stamps. */
function isStamped(node: Element): boolean {
  const properties = node.properties;
  return properties?.dataLineStart != null && properties?.dataLineEnd != null;
}

/**
 * True for a top-level element that rehype-katex will REPLACE wholesale (losing
 * its own `position` and every positioned descendant): a `<pre>` whose `<code>`
 * child carries the `language-math` class. This is the exact shape rehype-katex
 * swaps for a fresh `<span class="katex-display">` — it covers both `$$…$$`
 * (remark-math, which also tags the code `language-math`) and ` ```math ` code
 * fences. Inline math (`math-inline`) lives inside a `<p>` whose position katex
 * never touches, so it is deliberately excluded.
 */
function isBlockMathContainer(node: RootContent): node is Element {
  if (node.type !== 'element' || node.tagName !== 'pre') return false;
  return node.children.some(
    (child) =>
      child.type === 'element' &&
      child.tagName === 'code' &&
      Array.isArray(child.properties?.className) &&
      child.properties.className.includes('language-math'),
  );
}

/**
 * Pre-katex rehype step: wrap every top-level block-math element in a
 * pipeline-owned `<div>` stamped with the block's true source range, taken from
 * the element's still-present `position`.
 *
 * This is the provenance channel for math. rehype-katex rebuilds block math as a
 * brand-new node with no positional ancestry, so a stamp written onto the math
 * element itself would be discarded when katex replaces it. By wrapping the math
 * in a `<div data-line-start data-line-end>` BEFORE katex runs, katex rebuilds
 * *inside* the wrapper and the stamp — carried by the wrapper the pipeline owns —
 * survives. The final {@link stampSourceLines} pass then leaves these wrappers
 * untouched (they are already stamped) and never guesses a range by ordering.
 *
 * Runs AFTER rehype-sanitize — the sanitizer strips any user-supplied
 * `data-line-*`, so the only stamps that exist are the ones written here and by
 * the final pass; a raw `<div data-line-start=…>` can never forge provenance.
 * The math `<pre>` still carries its `position` here because sanitize preserves
 * node positions. Still runs BEFORE rehype-katex, which is all it needs.
 */
export function rehypeStampBlockMath() {
  return (tree: HastRoot): void => {
    tree.children = tree.children.map((node) => {
      if (!isBlockMathContainer(node) || !node.position) return node;
      const wrapper: Element = {
        type: 'element',
        tagName: 'div',
        properties: {
          dataLineStart: String(node.position.start.line),
          dataLineEnd: String(node.position.end.line),
        },
        children: [node],
      };
      return wrapper;
    });
  };
}

/**
 * The provenance contract: stamp every top-level rendered block with
 * `data-line-start` / `data-line-end` from its *true* source position.
 *
 * Position is transferred BY NODE, never zipped by index or matched by class:
 *  1. block math is stamped earlier, by {@link rehypeStampBlockMath}, onto a
 *     wrapper `<div>` the pipeline owns — those wrappers arrive here already
 *     stamped and are left untouched;
 *  2. every other top-level element is stamped from its own carried `position`
 *     (remark-rehype copies it onto markdown-origin nodes; rehype-raw assigns it
 *     to elements expanded from raw HTML — so a single raw block that becomes
 *     many elements stamps each one correctly and never shifts later blocks);
 *  3. a generated container with no `position` of its own (the hoisted footnotes
 *     `<section>`) derives its range from the min/max source lines of its
 *     positionally-anchored descendants.
 *
 * There is no ordering queue anywhere: an element with no positional ancestry of
 * its own is simply left unstamped (the corpus totality test asserts that set
 * stays empty). A positionless raw element that happens to carry a katex-ish
 * class can therefore never steal a real math block's range.
 *
 * These stamps are the provenance channel consumed by `scrollToLine`.
 */
export function stampSourceLines(hast: HastRoot): void {
  for (const node of hast.children) {
    if (node.type !== 'element') continue;
    if (isStamped(node)) continue; // block-math wrappers, stamped pre-katex

    const range = ownRange(node) ?? deriveRange(node);
    if (!range) continue;

    applyStamp(node, range);
  }
}
