import { unified } from 'unified';
import remarkParse from 'remark-parse';
import remarkGfm from 'remark-gfm';
import remarkFrontmatter from 'remark-frontmatter';
import remarkMath from 'remark-math';
import remarkRehype from 'remark-rehype';
import rehypeRaw from 'rehype-raw';
import rehypeSanitize from 'rehype-sanitize';
import rehypeSlug from 'rehype-slug';
import rehypeKatex from 'rehype-katex';
import type { Options as KatexOptions } from 'rehype-katex';
import { visit } from 'unist-util-visit';
import type { Element, Root as HastRoot } from 'hast';
import type { Heading, Root as MdastRoot } from 'mdast';

import { extractFrontmatter } from './frontmatter.js';
import { rehypeReconcileFragments } from './reconcile-fragments.js';
import { readerSanitizeSchema, USER_CONTENT_PREFIX } from './sanitize-schema.js';
import { rehypeStampBlockMath, stampSourceLines } from './source-lines.js';
import { rehypeUniqueIds } from './unique-ids.js';
import { remarkWikiLink } from './wikilink.js';
import type { OutlineItem, ParseOptions, ParsedDoc } from './types.js';

const HEADING_TAG = /^h[1-6]$/;

/** A unist node loosely viewed for text extraction (value and/or children). */
interface TextualNode {
  type: string;
  value?: unknown;
  children?: unknown[];
}

/**
 * Rendered text of an MDAST heading, for the outline. Concatenates the `value`
 * of every descendant that carries one — which, crucially, includes the raw TeX
 * of `inlineMath` nodes (remark-math stores the source in `value`) and the text
 * of `inlineCode`, so a `$E = mc^2$` heading reads as its readable source rather
 * than the KaTeX glyph run (`E=mc2E = mc^2E=mc2`) the final hast would serialise.
 * Reading text from the MDAST — the pristine markdown parse — is also what makes
 * the outline spoof-proof: collection TRUNCATES at the first raw-HTML `html`
 * node encountered in document order (both its `value` and everything after it
 * in the whole subtree are excluded), so the label is always a pure
 * markdown-only PREFIX of the heading. This is deliberately stronger than
 * merely skipping each `html` node in place: `# Foo<h2 id="foo">Attacker</h2>`
 * has the markdown text "Foo" followed by an `html` open-tag node and then a
 * text node "Attacker" — skip-in-place would still concatenate "Foo" +
 * "Attacker" into "FooAttacker" (the attacker's text smuggled onto the real
 * label), whereas truncating at the first `html` node stops before
 * "Attacker" is ever reached, yielding "Foo". The same truncation applies to
 * ordinary inline raw HTML used as formatting, not just spoofed headings: e.g.
 * `# A <b>x</b> B` yields "A " (the source text up to, not including, the
 * `<b>` tag) rather than "A x B" — accepted and documented here rather than
 * worked around, since any raw HTML inside a heading is, by construction, not
 * part of the markdown-authored label.
 */
function mdastHeadingText(node: Heading): string {
  let out = '';
  let truncated = false;
  const walk = (current: TextualNode): void => {
    if (truncated) return;
    // The first raw-HTML fragment ends collection outright: nothing at or
    // after it — its own `value`, its children, or later siblings anywhere in
    // the subtree — is ever appended.
    if (current.type === 'html') {
      truncated = true;
      return;
    }
    if (typeof current.value === 'string') out += current.value;
    const children = current.children;
    if (Array.isArray(children)) {
      for (const child of children) {
        if (truncated) break;
        walk(child as TextualNode);
      }
    }
  };
  walk(node as unknown as TextualNode);
  return out;
}

/**
 * Build the unified processor. A fresh instance is created per parse so that
 * per-document state (e.g. the slugger used by rehype-slug) never leaks between
 * documents.
 *
 * Chain: parse -> gfm -> frontmatter -> math -> wikilink -> rehype -> raw ->
 * sanitize -> slug -> unique-ids -> stamp-block-math -> katex ->
 * reconcile-fragments. Ordering is load-bearing:
 *
 *  - `sanitize` runs FIRST among the rehype-html steps, so it strips any
 *    user-supplied `data-line-*` (provenance can't be forged from raw HTML) and
 *    every raw `data-*` (footnote-shaped attrs included), and, as the single
 *    prefix authority, namespaces every surviving `id` with `user-content-`.
 *    remark-rehype is run with `clobberPrefix: ''` so footnote ids/references
 *    arrive un-prefixed and the sanitizer prefixes each id once.
 *  - `slug` prefixes heading ids with the same `user-content-` namespace, so the
 *    outline/scroll-spy slugs (read from the final hast ids) line up with the
 *    rendered anchors — one source of truth.
 *  - `unique-ids` ({@link rehypeUniqueIds}) runs after every id source (sanitize,
 *    slug, footnotes) and BEFORE reconcile. It is ORIGIN-BLIND, first-wins: the
 *    first element to claim an id keeps it and every later duplicate is suffixed
 *    `-2`/`-3`…. It makes NO trust decision — by this point `rehype-raw` has
 *    merged trusted and untrusted nodes, so any per-node "who is real" signal
 *    would be forgeable; document order is the only unforgeable tiebreak. After
 *    it every id is unique, so reconciliation resolves any reference that
 *    matches a final id to exactly one element — never a duplicate. A
 *    reference matching no id (a dead anchor, or an external link) is left
 *    dangling, unchanged; not every fragment is guaranteed to resolve.
 *  - `stamp-block-math` runs AFTER sanitize (so its stamps survive) but BEFORE
 *    katex (while block math still carries its source position), wrapping each
 *    block-math element in a stamped `<div>` the pipeline owns.
 *  - `reconcile-fragments` runs LAST, once every id is final, rewriting the
 *    `href`/`aria-*` references the sanitizer left un-prefixed to the surviving
 *    prefixed ids (footnotes AND raw intra-document anchors).
 *
 * A final {@link stampSourceLines} pass in {@link parseMarkdown} then fills every
 * remaining top-level block from its own position (or, for generated containers,
 * its descendants). No ordering/queue heuristic is used anywhere.
 */
function buildProcessor(opts: ParseOptions) {
  // rehype-katex never throws on malformed math (it renders the source in
  // `errorColor`), so there is no throwOnError to configure.
  const katexOptions: KatexOptions = {};
  if (opts.katexErrorColor) katexOptions.errorColor = opts.katexErrorColor;

  return unified()
    .use(remarkParse)
    .use(remarkGfm)
    .use(remarkFrontmatter, ['yaml'])
    .use(remarkMath)
    .use(remarkWikiLink)
    // clobberPrefix '' -> footnote ids/references arrive un-prefixed; the
    // sanitizer below is the single authority that applies `user-content-`.
    .use(remarkRehype, { allowDangerousHtml: true, clobberPrefix: '' })
    .use(rehypeRaw)
    .use(rehypeSanitize, readerSanitizeSchema)
    .use(rehypeSlug, { prefix: USER_CONTENT_PREFIX })
    .use(rehypeUniqueIds)
    .use(rehypeStampBlockMath)
    .use(rehypeKatex, katexOptions)
    .use(rehypeReconcileFragments);
}

/**
 * Build the heading outline by EXACT source-position matching (spoof-proof).
 *
 * Entries are computed FROM the MDAST headings — the pristine markdown parse,
 * which by construction contains only real markdown headings (a raw-HTML `<h2>`
 * is an `html` node here, never a heading). For each MDAST heading we take its
 * depth and text from the MDAST and then look up its rendered hast heading by
 * EXACT start-position equality (line AND column): two distinct source spans
 * cannot share a start offset, so a same-line raw `<h2>` — whose start column
 * differs — can never match a markdown heading's position and never becomes an
 * entry. The entry's `slug` is read off the matched node's FINAL (post-dedup)
 * `id`, so an origin-blind suffix on the heading is reflected faithfully. A
 * markdown heading with no position-matched hast node (should not happen for a
 * real heading) is omitted rather than guessed.
 */
function buildOutline(mdast: MdastRoot, hast: HastRoot): OutlineItem[] {
  // Index rendered hast headings by their exact source start `line:column`.
  // First-wins on key collisions is irrelevant in practice (distinct spans have
  // distinct starts), but keeping the first is the safe, deterministic choice.
  const hastHeadingByStart = new Map<string, Element>();
  visit(hast, 'element', (node: Element) => {
    if (!HEADING_TAG.test(node.tagName)) return;
    const start = node.position?.start;
    if (start == null || typeof start.line !== 'number' || typeof start.column !== 'number') return;
    const key = `${start.line}:${start.column}`;
    if (!hastHeadingByStart.has(key)) hastHeadingByStart.set(key, node);
  });

  const items: OutlineItem[] = [];
  visit(mdast, 'heading', (node: Heading) => {
    const start = node.position?.start;
    if (start == null || typeof start.line !== 'number' || typeof start.column !== 'number') return;
    const match = hastHeadingByStart.get(`${start.line}:${start.column}`);
    if (match == null) return; // no position-matched node -> omit (never guess)
    const id = match.properties?.id;
    items.push({
      depth: node.depth,
      text: mdastHeadingText(node),
      slug: typeof id === 'string' ? id : '',
      lineStart: start.line,
    });
  });
  return items;
}

/**
 * Parse markdown into the single {@link ParsedDoc} shape shared verbatim by the
 * desktop app and the website. Synchronous; never throws on malformed input
 * (bad YAML, unclosed fences, broken wikilinks, invalid math all degrade
 * gracefully).
 */
export function parseMarkdown(source: string, opts: ParseOptions = {}): ParsedDoc {
  const processor = buildProcessor(opts);
  const mdast = processor.parse(source) as MdastRoot;
  const hast = processor.runSync(mdast) as unknown as HastRoot;

  stampSourceLines(hast);

  const frontmatter = extractFrontmatter(mdast);
  // Outline is matched from the pristine MDAST headings to their rendered hast
  // nodes by exact source position, so raw-HTML headings never enter it.
  const outline = buildOutline(mdast, hast);

  const blockCount = hast.children.filter((child) => child.type === 'element').length;
  const byteLength = new TextEncoder().encode(source).length;

  return {
    hast,
    frontmatter,
    outline,
    stats: { blockCount, byteLength },
  };
}
