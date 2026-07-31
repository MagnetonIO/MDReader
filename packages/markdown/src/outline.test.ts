import { visit } from 'unist-util-visit';
import { describe, expect, test } from 'vitest';
import type { Element, Root } from 'hast';

import { parseMarkdown } from './pipeline.js';
import { readCorpus } from './test-utils.js';

/** Map every heading id present in the rendered hast. */
function headingIds(hast: Root): string[] {
  const ids: string[] = [];
  visit(hast, 'element', (node: Element) => {
    if (/^h[1-6]$/.test(node.tagName) && typeof node.properties?.id === 'string') {
      ids.push(node.properties.id);
    }
  });
  return ids;
}

describe('outline extraction', () => {
  test('nested headings produce a correctly ordered, depth-aware outline', () => {
    const doc = parseMarkdown(readCorpus('docs/reference/headings-deep.md'));
    // Six real headings; the seventh-level `#######` line is not a heading.
    expect(doc.outline.map((item) => item.depth)).toEqual([1, 2, 3, 4, 5, 6]);
    expect(doc.outline.map((item) => item.text)).toEqual([
      'H1 — Top Level',
      'H2 — Section',
      'H3 — Subsection',
      'H4 — Sub-subsection',
      'H5 — Deeper',
      'H6 — Deepest',
    ]);
    // Each outline line points at the real source line of its heading.
    expect(doc.outline.map((item) => item.lineStart)).toEqual([6, 10, 14, 18, 22, 26]);
  });

  test('duplicate headings get deduplicated slugs matching the DOM ids', () => {
    const doc = parseMarkdown(readCorpus('docs/reference/headings-duplicate.md'));
    const slugs = doc.outline.map((item) => item.slug);
    // Heading ids are namespaced with `user-content-` (rehype-slug prefix), which
    // is the same value the sanitizer/reconcile use — so anchors, outline slugs,
    // and scroll-spy registration all share one string. Dedup suffixes are added
    // after the prefix.
    expect(slugs).toEqual([
      'user-content-overview',
      'user-content-details',
      'user-content-details-1',
      'user-content-details-2',
      'user-content-overview-1',
    ]);
    // Outline slugs must equal the rendered heading ids (scroll-spy contract).
    expect(slugs).toEqual(headingIds(doc.hast));
  });

  test('outline slugs equal DOM ids even for inline-markup headings', () => {
    const doc = parseMarkdown(readCorpus('docs/reference/headings-with-inline.md'));
    expect(doc.outline.map((item) => item.slug)).toEqual(headingIds(doc.hast));
    expect(doc.outline[0]?.text).toBe('Using catdb-query in Practice');
  });

  test('a document that starts at h3 keeps the real depth', () => {
    const doc = parseMarkdown(readCorpus('docs/reference/headings-start-h3.md'));
    expect(doc.outline[0]?.depth).toBe(3);
  });

  test('a document with no headings has an empty outline', () => {
    const doc = parseMarkdown(readCorpus('docs/reference/no-headings.md'));
    expect(doc.outline).toEqual([]);
  });

  test('a raw <h2> on its own line is excluded; only the markdown heading appears', () => {
    // The outline is computed from the pristine MDAST headings (a raw-HTML `<h2>`
    // is an `html` node there, NOT a heading) and matched to the rendered hast by
    // exact source position. The raw heading has no MDAST-heading counterpart, so
    // it never becomes an entry; the markdown heading matches its own hast node
    // and reads ITS OWN final id.
    const doc = parseMarkdown(
      ['<h2 id="raw">Raw Heading</h2>', '', '# Real Heading', '', 'Body.'].join('\n'),
    );

    expect(doc.outline).toHaveLength(1);
    expect(doc.outline[0]).toMatchObject({
      depth: 1,
      text: 'Real Heading',
      slug: 'user-content-real-heading',
    });
    // The raw heading's id is not what the sole outline entry points at.
    expect(doc.outline[0]?.slug).not.toBe('user-content-raw');
  });

  test('`# Foo<h2 id="foo">Attacker</h2>`: exactly one entry, the markdown heading', () => {
    // A same-line raw `<h2>` smuggled into a markdown heading. It shares the
    // heading's LINE but not its start COLUMN, so it can never match the MDAST
    // heading's exact start position. The outline therefore has exactly one entry
    // — the markdown heading, reading its own true final id — and the attacker
    // heading is never present.
    const doc = parseMarkdown(['# Foo<h2 id="foo">Attacker</h2>', '', 'Body.'].join('\n'));

    expect(doc.outline).toHaveLength(1);
    expect(doc.outline[0]?.depth).toBe(1);
    expect(doc.outline[0]?.lineStart).toBe(1);
    // The label is the markdown-only PREFIX: text collection truncates at the
    // first raw-HTML node in the heading, so the attacker's "Attacker" text
    // (which sits, in document order, after the `<h2 id="foo">` open-tag node)
    // is never reached, let alone concatenated onto "Foo".
    expect(doc.outline[0]?.text).toBe('Foo');
    // The sole entry's slug is the markdown heading's FINAL id (an existing id in
    // the tree), and never the attacker's suffixed id.
    const headingIdSet = new Set(headingIds(doc.hast));
    expect(headingIdSet.has(doc.outline[0]!.slug)).toBe(true);
    expect(doc.outline[0]?.slug).not.toBe('user-content-foo-2');
  });

  test('raw inline HTML mid-heading truncates the label at the tag (documented, not a spoof case)', () => {
    // `<b>x</b>` is ordinary inline formatting, but remark parses it as two
    // `html` nodes around a `text` node — the same node type a spoofed heading
    // uses. Truncating at the FIRST `html` node is what makes the spoof case
    // above safe, and it applies uniformly here too: the label is the raw
    // markdown source up to (not including) the `<b>` tag, so trailing
    // whitespace before the tag is preserved verbatim ("A ", not "A") since this
    // function does no whitespace normalization — it only concatenates node
    // `value`s in document order and stops at the first `html` node.
    const doc = parseMarkdown(['# A <b>x</b> B', '', 'Body.'].join('\n'));

    expect(doc.outline).toHaveLength(1);
    expect(doc.outline[0]?.text).toBe('A ');
  });

  test('outline text survives inline math (reads the TeX source, not katex glyphs)', () => {
    const doc = parseMarkdown(readCorpus('docs/reference/headings-with-inline.md'));
    const mathHeading = doc.outline.find((item) => item.depth === 4);
    // The final hast renders `$E = mc^2$` as katex spans; the outline reads the
    // TeX `<annotation>` so the entry text stays the readable source.
    expect(mathHeading?.text).toBe('Math in a heading: E = mc^2');
  });
});
