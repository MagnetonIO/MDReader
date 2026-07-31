import { visit } from 'unist-util-visit';
import { describe, expect, test } from 'vitest';
import type { Element } from 'hast';

import { parseMarkdown } from './pipeline.js';
import { readCorpus } from './test-utils.js';

interface WikiInfo {
  target?: string;
  heading?: string;
  label?: string;
  text?: string;
}

function wikiLinks(source: string): WikiInfo[] {
  const doc = parseMarkdown(source);
  const links: WikiInfo[] = [];
  visit(doc.hast, 'element', (node: Element) => {
    if (node.tagName !== 'wiki-link') return;
    const child = node.children[0];
    links.push({
      target: node.properties?.dataTarget as string | undefined,
      heading: node.properties?.dataHeading as string | undefined,
      label: node.properties?.dataLabel as string | undefined,
      text: child?.type === 'text' ? child.value : undefined,
    });
  });
  return links;
}

describe('wikilink micromark extension', () => {
  test('parses plain, labelled, heading, and heading+label targets', () => {
    const links = wikiLinks(readCorpus('notes/zettel/wikilinks.md'));
    expect(links).toEqual([
      { target: 'node-storage', heading: undefined, label: undefined, text: 'node-storage' },
      {
        target: 'node-query-engine',
        heading: undefined,
        label: 'the query engine',
        text: 'the query engine',
      },
      { target: 'node-search', heading: 'Search', label: undefined, text: 'node-search#Search' },
      {
        target: 'node-embeddings',
        heading: 'Embeddings',
        label: 'how vectors work',
        text: 'how vectors work',
      },
      { target: 'graph-hub', heading: undefined, label: undefined, text: 'graph-hub' },
      { target: 'node-ingest', heading: undefined, label: undefined, text: 'node-ingest' },
      { target: 'node-provenance', heading: undefined, label: undefined, text: 'node-provenance' },
      { target: 'node-graph-view', heading: undefined, label: undefined, text: 'node-graph-view' },
      { target: 'broken-links', heading: undefined, label: 'broken ones', text: 'broken ones' },
    ]);
  });

  test('broken/unresolved targets still produce wiki-link nodes', () => {
    // These targets do not resolve to any note, but the parser must still emit
    // nodes (resolution/styling is a rendering concern).
    const links = wikiLinks('A [[does-not-exist]] and a [[also#missing|label]].\n');
    expect(links).toEqual([
      { target: 'does-not-exist', heading: undefined, label: undefined, text: 'does-not-exist' },
      { target: 'also', heading: 'missing', label: 'label', text: 'label' },
    ]);
  });

  test('malformed openings fall back to literal text (no hang, no node)', () => {
    // Unterminated, empty, and nested-bracket forms are not wikilinks.
    expect(wikiLinks('Unterminated [[oops and then nothing.\n')).toEqual([]);
    expect(wikiLinks('Empty [[]] here.\n')).toEqual([]);
    expect(wikiLinks('A single [not a wiki] link.\n')).toEqual([]);
  });

  test('wiki-link elements carry className and data-wikilink marker', () => {
    const doc = parseMarkdown('See [[node-storage]].\n');
    let found: Element | undefined;
    visit(doc.hast, 'element', (node: Element) => {
      if (node.tagName === 'wiki-link') found = node;
    });
    expect(found?.properties?.className).toContain('wikilink');
    expect(found?.properties?.dataWikilink).toBe('true');
  });
});
