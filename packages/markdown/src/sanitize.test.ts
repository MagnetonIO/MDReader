import { unified } from 'unified';
import rehypeSanitize from 'rehype-sanitize';
import { visit } from 'unist-util-visit';
import { describe, expect, test } from 'vitest';
import type { Element, Properties, Root } from 'hast';

import { parseMarkdown } from './pipeline.js';
import { readerSanitizeSchema } from './sanitize-schema.js';

/** Element helper: a hast element with camelCase properties. */
function el(tagName: string, properties: Properties, children: Element['children'] = []): Element {
  return { type: 'element', tagName, properties, children };
}

/**
 * Run *only* rehype-sanitize with the reader schema over a crafted hast tree.
 * This exercises the schema in isolation — no slug/katex/stamp passes — so the
 * assertions genuinely reflect what the schema keeps and drops (the real
 * pipeline adds ids/katex/stamps *after* sanitize, which would otherwise mask
 * whether the schema itself retains them).
 */
function sanitizeTree(tree: Root): Root {
  return unified().use(rehypeSanitize, readerSanitizeSchema).runSync(tree) as Root;
}

function findByTag(tree: Root, tag: string): Element | undefined {
  let found: Element | undefined;
  visit(tree, 'element', (node: Element) => {
    if (!found && node.tagName === tag) found = node;
  });
  return found;
}

describe('sanitizer schema (in isolation)', () => {
  test('strips user-supplied data-line-* stamps (provenance cannot be forged)', () => {
    const tree: Root = {
      type: 'root',
      children: [el('p', { dataLineStart: '1', dataLineEnd: '1' }, [{ type: 'text', value: 'x' }])],
    };
    // The schema no longer allows data-line-*. Any raw stamp is dropped here; the
    // pipeline re-stamps every block from its TRUE position downstream of this
    // pass (rehypeStampBlockMath + the final stampSourceLines), so a raw
    // `<div data-line-start=1>` can never hijack scroll/highlight.
    const p = findByTag(sanitizeTree(tree), 'p');
    expect(p?.properties?.dataLineStart).toBeUndefined();
    expect(p?.properties?.dataLineEnd).toBeUndefined();
  });

  test('strips footnote-shaped data-* (data-footnotes / -ref / -backref) but keeps class', () => {
    const tree: Root = {
      type: 'root',
      children: [
        el('section', { dataFootnotes: true, className: ['footnotes'] }, [
          { type: 'text', value: 's' },
        ]),
        el('a', { dataFootnoteRef: true, href: '#x' }, [{ type: 'text', value: 'r' }]),
        el(
          'a',
          { dataFootnoteBackref: true, className: ['data-footnote-backref'], href: '#y' },
          [{ type: 'text', value: 'b' }],
        ),
      ],
    };
    const out = sanitizeTree(tree);
    const section = findByTag(out, 'section');
    // The forgeable footnote *marker* is gone; the (non-data) class survives.
    expect(section?.properties?.dataFootnotes).toBeUndefined();
    expect(section?.properties?.className).toEqual(['footnotes']);
    const anchors = out.children.filter((c): c is Element => c.type === 'element' && c.tagName === 'a');
    expect(anchors[0]?.properties?.dataFootnoteRef).toBeUndefined();
    expect(anchors[1]?.properties?.dataFootnoteBackref).toBeUndefined();
    // `class="data-footnote-backref"` is a className value, not a data attr — kept.
    expect(anchors[1]?.properties?.className).toEqual(['data-footnote-backref']);
  });

  test('strips arbitrary raw data-* attributes (empty data-* keep-list for raw content)', () => {
    const tree: Root = {
      type: 'root',
      children: [el('div', { dataEvil: 'x', dataFoo: 'y' }, [{ type: 'text', value: 'z' }])],
    };
    const div = findByTag(sanitizeTree(tree), 'div');
    expect(div?.properties?.dataEvil).toBeUndefined();
    expect(div?.properties?.dataFoo).toBeUndefined();
  });

  test('retains code className markers KaTeX/mermaid/highlighting read', () => {
    const tree: Root = {
      type: 'root',
      children: [
        el('code', { className: ['language-math', 'math-display'] }),
        el('code', { className: ['language-mermaid'] }),
        el('code', { className: ['language-rust'] }),
      ],
    };
    const out = sanitizeTree(tree);
    const classes = out.children
      .filter((c): c is Element => c.type === 'element')
      .map((c) => c.properties?.className);
    expect(classes).toEqual([['language-math', 'math-display'], ['language-mermaid'], ['language-rust']]);
  });

  test('retains and namespaces id on headings (slug/scroll-spy target)', () => {
    const tree: Root = {
      type: 'root',
      children: [el('h2', { id: 'my-heading' }, [{ type: 'text', value: 'H' }])],
    };
    // The sanitizer is the single prefix authority: `clobberPrefix` is
    // `user-content-`, so a surviving id is kept AND namespaced exactly once.
    expect(findByTag(sanitizeTree(tree), 'h2')?.properties?.id).toBe('user-content-my-heading');
  });

  test('namespaces raw id and drops name entirely (payloads cannot land bare)', () => {
    const tree: Root = {
      type: 'root',
      children: [
        el('a', { id: 'constructor' }, [{ type: 'text', value: 'x' }]),
        el('a', { name: 'constructor' }, [{ type: 'text', value: 'y' }]),
      ],
    };
    const out = sanitizeTree(tree);
    const anchors = out.children.filter((c): c is Element => c.type === 'element');
    // `id` is clobber-prefixed, so `id="constructor"` never reaches the DOM
    // unprefixed (no `window.constructor` clobbering, no collision with a real
    // slug/footnote id).
    expect(anchors[0]?.properties?.id).toBe('user-content-constructor');
    // `name` is no longer allowed at all — the legacy anchor mechanism is gone,
    // so a raw `name="constructor"` is dropped outright, not merely namespaced.
    expect(anchors[1]?.properties?.name).toBeUndefined();
  });

  test('drops className on non-code elements (no blanket grant)', () => {
    const tree: Root = {
      type: 'root',
      children: [
        el('p', { className: ['evil'] }, [{ type: 'text', value: 'x' }]),
        el('span', { className: ['evil'] }),
        el('div', { className: ['evil'] }),
      ],
    };
    const out = sanitizeTree(tree);
    for (const child of out.children) {
      if (child.type === 'element') expect(child.properties?.className).toBeUndefined();
    }
  });

  test('drops id on non-heading, non-footnote elements (no blanket grant)', () => {
    const tree: Root = {
      type: 'root',
      children: [el('div', { id: 'clobber-target' }, [el('p', { id: 'nope' })])],
    };
    const out = sanitizeTree(tree);
    expect(findByTag(out, 'div')?.properties?.id).toBeUndefined();
    expect(findByTag(out, 'p')?.properties?.id).toBeUndefined();
  });

  test('restricts <wiki-link> className to the wikilink token (no open grant)', () => {
    const tree: Root = {
      type: 'root',
      children: [
        el('wiki-link', { className: ['wikilink', 'katex-display', 'math-display', 'evil'] }, [
          { type: 'text', value: 'x' },
        ]),
      ],
    };
    // Only the legitimate `wikilink` token survives; a smuggled math/katex class
    // (which the block-math stamper keys off of) is stripped.
    expect(findByTag(sanitizeTree(tree), 'wiki-link')?.properties?.className).toEqual(['wikilink']);
  });
});

const DIRTY = [
  '# Title',
  '',
  '<script>alert("xss")</script>',
  '',
  '<p onclick="steal()">Click me</p>',
  '',
  'Inline math $a^2+b^2=c^2$ stays intact.',
  '',
  '[bad link](javascript:evil())',
  '',
  'See [[node-storage#Search|the store]] too.',
  '',
].join('\n');

function collectElements(source: string): Element[] {
  const doc = parseMarkdown(source);
  const elements: Element[] = [];
  visit(doc.hast, 'element', (node: Element) => {
    elements.push(node);
  });
  return elements;
}

describe('sanitizer pipeline (end-to-end)', () => {
  test('strips <script> elements', () => {
    expect(collectElements(DIRTY).some((elm) => elm.tagName === 'script')).toBe(false);
  });

  test('strips inline event handlers (onclick)', () => {
    const hasHandler = collectElements(DIRTY).some((elm) =>
      Object.keys(elm.properties ?? {}).some((key) => /^on/i.test(key)),
    );
    expect(hasHandler).toBe(false);
  });

  test('strips javascript: URLs', () => {
    const hasJsUrl = collectElements(DIRTY).some(
      (elm) =>
        elm.tagName === 'a' &&
        typeof elm.properties?.href === 'string' &&
        elm.properties.href.toLowerCase().startsWith('javascript:'),
    );
    expect(hasJsUrl).toBe(false);
  });

  test('leaves provenance stamps intact on the surviving output', () => {
    const doc = parseMarkdown(DIRTY);
    const heading = doc.hast.children.find(
      (child): child is Element => child.type === 'element' && child.tagName === 'h1',
    );
    expect(heading?.properties?.dataLineStart).toBe('1');
    expect(heading?.properties?.dataLineEnd).toBe('1');
  });

  test('keeps rendered KaTeX spans', () => {
    const hasKatex = collectElements(DIRTY).some((elm) => {
      const className = elm.properties?.className;
      return Array.isArray(className) && className.some((c) => String(c).startsWith('katex'));
    });
    expect(hasKatex).toBe(true);
  });

  test('keeps custom <wiki-link> elements and their data attributes', () => {
    const wiki = collectElements('See [[node-storage#Search|the store]] for details.\n').find(
      (elm) => elm.tagName === 'wiki-link',
    );
    expect(wiki?.properties?.dataTarget).toBe('node-storage');
    expect(wiki?.properties?.dataHeading).toBe('Search');
    expect(wiki?.properties?.dataLabel).toBe('the store');
  });

  test('genuine GFM footnote output no longer carries data-footnote* markers', () => {
    // Real footnotes still render and resolve (see pipeline.test.ts), but the
    // sanitizer strips their forgeable `data-footnotes`/`data-footnote-ref`/
    // `data-footnote-backref` markers — nothing in the reader consumes them, and
    // dropping them removes a shape a raw block could imitate.
    const source = ['A claim.[^1]', '', '[^1]: The footnote body.', ''].join('\n');
    for (const elm of collectElements(source)) {
      expect(elm.properties?.dataFootnotes).toBeUndefined();
      expect(elm.properties?.dataFootnoteRef).toBeUndefined();
      expect(elm.properties?.dataFootnoteBackref).toBeUndefined();
    }
  });
});
