import { visit } from 'unist-util-visit';
import { describe, expect, test } from 'vitest';
import type { Element, Root } from 'hast';

import { parseMarkdown } from './pipeline.js';

/** All ids present in the final tree, in document order. */
function ids(hast: Root): string[] {
  const out: string[] = [];
  visit(hast, 'element', (node: Element) => {
    if (typeof node.properties?.id === 'string') out.push(node.properties.id);
  });
  return out;
}

/** The single element carrying a given id (asserts uniqueness). */
function elementById(hast: Root, id: string): Element {
  const matches: Element[] = [];
  visit(hast, 'element', (node: Element) => {
    if (node.properties?.id === id) matches.push(node);
  });
  expect(matches).toHaveLength(1);
  return matches[0]!;
}

/** All `#`-fragment hrefs in the final tree. */
function fragmentHrefs(hast: Root): string[] {
  const out: string[] = [];
  visit(hast, 'element', (node: Element) => {
    const href = node.properties?.href;
    if (typeof href === 'string' && href.startsWith('#')) out.push(href);
  });
  return out;
}

/**
 * Structural guarantees checked here: (a) on ANY input, every id in the final
 * tree is unique; (b) fragment references that match a final id resolve to
 * exactly one element (dangling references are allowed by the pipeline and
 * preserved unchanged — the fixtures used with this helper simply contain
 * none, so it asserts all of THEIR fragments resolve). Nothing about *which*
 * element keeps a bare id is asserted — that is document-order and
 * deliberately not a trust decision.
 */
function assertUniqueAndResolving(hast: Root): void {
  const all = ids(hast);
  const unique = new Set(all);
  expect(all.length).toBe(unique.size); // no duplicate ids anywhere
  for (const href of fragmentHrefs(hast)) {
    expect(unique.has(href.slice(1))).toBe(true); // every fragment resolves
  }
}

describe('origin-blind, first-wins id dedup', () => {
  test('first occurrence in document order keeps the bare id; later duplicates are suffixed', () => {
    // The raw anchor appears BEFORE the markdown heading, so — origin-blind — the
    // raw anchor keeps `user-content-foo` and the heading is suffixed. This is the
    // accepted, documented consequence: no trust signal decides the winner, only
    // document order (unforgeable), and every reference still resolves uniquely.
    const doc = parseMarkdown(
      ['<a id="foo">raw anchor</a>', '', '# Foo', '', '[go](#foo)'].join('\n'),
    );

    expect(elementById(doc.hast, 'user-content-foo').tagName).toBe('a');
    expect(elementById(doc.hast, 'user-content-foo-2').tagName).toBe('h1');
    // Exactly one element owns the un-suffixed id (no DOM duplicate).
    expect(ids(doc.hast).filter((id) => id === 'user-content-foo')).toHaveLength(1);
    // The `#foo` link resolves to the surviving first occurrence.
    expect(fragmentHrefs(doc.hast)).toContain('#user-content-foo');
    expect(fragmentHrefs(doc.hast)).not.toContain('#user-content-foo-2');
    assertUniqueAndResolving(doc.hast);
  });

  test('two raw ids with the same name are suffixed deterministically (-2, -3)', () => {
    const doc = parseMarkdown(
      ['<a id="dup">a</a>', '', '<a id="dup">b</a>', '', '<a id="dup">c</a>'].join('\n'),
    );
    // The FIRST occurrence keeps the base name; the rest are suffixed in order.
    expect(ids(doc.hast)).toEqual([
      'user-content-dup',
      'user-content-dup-2',
      'user-content-dup-3',
    ]);
  });
});

describe('prior attacks now yield deterministic, unique, resolving ids', () => {
  test('forged <section data-footnotes> shape cannot break id uniqueness', () => {
    // A real footnote AND a hand-forged footnote section colliding on `fn-1`. The
    // sanitizer strips the forgeable `data-footnotes` marker (see sanitize.test),
    // and dedup is origin-blind: whatever the forge, ids stay unique and every
    // footnote/anchor reference resolves to exactly one element.
    const doc = parseMarkdown(
      [
        'A claim.[^1]',
        '',
        '[^1]: The real footnote body.',
        '',
        '<section data-footnotes class="footnotes"><ol><li id="fn-1"><p>Forged body.</p></li></ol></section>',
        '',
        '[see](#fn-1)',
      ].join('\n'),
    );

    // Both `fn-1` claimants survive under distinct final ids.
    expect(ids(doc.hast)).toContain('user-content-fn-1');
    expect(ids(doc.hast)).toContain('user-content-fn-1-2');
    // No raw `<section>` retained the forgeable footnote marker.
    visit(doc.hast, 'element', (node: Element) => {
      expect(node.properties?.dataFootnotes).toBeUndefined();
    });
    assertUniqueAndResolving(doc.hast);
  });

  test('`# Foo<h2 id="foo">Attacker</h2>`: same-line raw heading cannot break uniqueness', () => {
    const doc = parseMarkdown(['# Foo<h2 id="foo">Attacker</h2>', '', '[go](#foo)'].join('\n'));

    // The markdown h1 comes first in document order and keeps `user-content-foo`;
    // the attacker h2 is suffixed. (The attacker never enters the outline — see
    // outline.test.ts — because its source start position differs.)
    expect(elementById(doc.hast, 'user-content-foo').tagName).toBe('h1');
    expect(elementById(doc.hast, 'user-content-foo-2').tagName).toBe('h2');
    assertUniqueAndResolving(doc.hast);
  });
});

describe('double-prefix invariant (reconcile resolves to exactly one final id)', () => {
  test('a raw pre-prefixed id="user-content-x" resolves (prefix applied once, not twice)', () => {
    const doc = parseMarkdown(
      ['<a id="user-content-x">raw</a>', '', '[x](#user-content-x)'].join('\n'),
    );

    // Sanitize prefixes unconditionally, so the raw id legitimately doubles.
    const anchor = elementById(doc.hast, 'user-content-user-content-x');
    expect(anchor.tagName).toBe('a');

    // The reference is prefixed exactly ONCE to match the (doubled) final id —
    // resolution survives, and the target exists as a real id.
    const href = fragmentHrefs(doc.hast).find((h) => h.includes('user-content-x'));
    expect(href).toBe('#user-content-user-content-x');
    expect(ids(doc.hast)).toContain(href!.slice(1));
  });

  test('every internal fragment reference resolves to exactly one final id', () => {
    // A mix of markdown link, raw anchor collision, footnote, and pre-prefixed id.
    const doc = parseMarkdown(
      [
        '<a id="foo">raw</a>',
        '',
        '# Foo',
        '',
        'A note.[^1]',
        '',
        '[^1]: Body.',
        '',
        '[to foo](#foo)',
      ].join('\n'),
    );
    assertUniqueAndResolving(doc.hast);
  });
});
