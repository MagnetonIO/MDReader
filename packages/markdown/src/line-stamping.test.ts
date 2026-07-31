import { describe, expect, test } from 'vitest';
import { visit } from 'unist-util-visit';
import type { Element, Root } from 'hast';

import { parseMarkdown } from './pipeline.js';
import { listCorpusMarkdown, readCorpus } from './test-utils.js';

interface Block {
  tag: string;
  start: string | undefined;
  end: string | undefined;
}

/** Extract the top-level blocks with their `data-line-*` stamps. */
function topBlocks(hast: Root): Block[] {
  return hast.children
    .filter((child): child is Element => child.type === 'element')
    .map((element) => ({
      tag: element.tagName,
      start: element.properties?.dataLineStart as string | undefined,
      end: element.properties?.dataLineEnd as string | undefined,
    }));
}

/**
 * Golden line-stamping test. Values were hand-verified once against the source
 * and are now locked. This is the provenance contract: line ranges, not slugs.
 */
describe('source-line provenance stamps', () => {
  test('code-fences.md stamps every top-level block with its source lines', () => {
    const blocks = topBlocks(parseMarkdown(readCorpus('docs/guides/code-fences.md')).hast);
    expect(blocks).toEqual([
      { tag: 'h1', start: '6', end: '6' },
      { tag: 'p', start: '8', end: '9' },
      { tag: 'h2', start: '11', end: '11' },
      { tag: 'pre', start: '13', end: '17' }, // ```rust
      { tag: 'h2', start: '19', end: '19' },
      { tag: 'pre', start: '21', end: '24' }, // ```python
      { tag: 'h2', start: '26', end: '26' },
      { tag: 'pre', start: '28', end: '32' }, // ```typescript
      { tag: 'h2', start: '34', end: '34' },
      { tag: 'pre', start: '36', end: '39' }, // ```sql
      { tag: 'h2', start: '41', end: '41' },
      { tag: 'pre', start: '43', end: '47' }, // ```bash
      { tag: 'h2', start: '49', end: '49' },
      { tag: 'pre', start: '51', end: '53' }, // ```json
      { tag: 'h2', start: '55', end: '55' },
      { tag: 'pre', start: '57', end: '61' }, // ```toml
      { tag: 'h2', start: '63', end: '63' },
      { tag: 'pre', start: '65', end: '66' }, // indented code
      { tag: 'p', start: '68', end: '68' },
      { tag: 'pre', start: '70', end: '72' }, // ~~~text
    ]);
  });

  test('katex-block.md stamps block math (which loses its own position) correctly', () => {
    const blocks = topBlocks(parseMarkdown(readCorpus('docs/guides/katex-block.md')).hast);
    // Block math is wrapped pre-katex in a pipeline-owned <div> that carries the
    // stamp, because katex rebuilds the math with no positional ancestry.
    expect(blocks).toEqual([
      { tag: 'h1', start: '6', end: '6' },
      { tag: 'p', start: '8', end: '8' },
      { tag: 'div', start: '10', end: '12' }, // $$ softmax $$
      { tag: 'p', start: '14', end: '14' },
      { tag: 'div', start: '16', end: '21' }, // $$ aligned $$
      { tag: 'p', start: '23', end: '23' },
      { tag: 'div', start: '25', end: '27' }, // $$ matrix $$
      { tag: 'p', start: '29', end: '30' },
    ]);
  });

  test('gfm-tables.md stamps tables with their source line ranges', () => {
    const blocks = topBlocks(parseMarkdown(readCorpus('docs/guides/gfm-tables.md')).hast);
    expect(blocks).toEqual([
      { tag: 'h1', start: '6', end: '6' },
      { tag: 'p', start: '8', end: '8' },
      { tag: 'table', start: '10', end: '13' },
      { tag: 'p', start: '15', end: '16' },
      { tag: 'table', start: '18', end: '23' },
      { tag: 'p', start: '25', end: '25' },
      { tag: 'table', start: '27', end: '30' },
      { tag: 'p', start: '32', end: '32' },
    ]);
  });

  test('footnotes.md stamps the hoisted footnotes <section> from its descendants', () => {
    const blocks = topBlocks(parseMarkdown(readCorpus('docs/guides/footnotes.md')).hast);
    expect(blocks).toEqual([
      { tag: 'h1', start: '6', end: '6' },
      { tag: 'p', start: '8', end: '10' },
      { tag: 'p', start: '12', end: '12' },
      { tag: 'p', start: '23', end: '23' },
      // The footnotes section carries no position of its own; its range is
      // derived from the min/max source lines of the hoisted definitions.
      { tag: 'section', start: '14', end: '21' },
    ]);
  });
});

/**
 * Totality of the provenance contract: *every* top-level element across the
 * whole corpus must carry `data-line-start`/`data-line-end`. The allowed set of
 * unstampable top-level tags is empty and must stay empty.
 */
describe('source-line stamping totality', () => {
  const ALLOWED_UNSTAMPABLE = new Set<string>(); // goal: empty

  test.each(listCorpusMarkdown())('%s stamps every top-level element', (fixture) => {
    const blocks = topBlocks(parseMarkdown(readCorpus(fixture)).hast);
    const unstamped = blocks.filter(
      (block) =>
        (block.start === undefined || block.end === undefined) &&
        !ALLOWED_UNSTAMPABLE.has(block.tag),
    );
    expect(unstamped).toEqual([]);
  });
});

/**
 * Regression for the one-to-many raw-HTML expansion that broke index-zipping:
 * a single mdast `html` block that rehype-raw expands into several top-level
 * elements must not shift the source lines of any later block. Each element is
 * stamped from its own carried position instead.
 */
describe('raw-HTML one-to-many alignment', () => {
  test('a multi-element raw block does not misalign later blocks', () => {
    const source = [
      '# Title', // line 1
      '', // 2
      'Before.', // 3
      '', // 4
      // one mdast html node -> two top-level <img> elements after rehype-raw
      '<figure><img src="a.png"></figure><figure><img src="b.png"></figure>', // 5
      '', // 6
      'After para.', // 7
      '', // 8
      'Last.', // 9
    ].join('\n');

    const blocks = topBlocks(parseMarkdown(source).hast);

    // Both expanded elements point at the raw block's own line (5); the two
    // trailing paragraphs keep their true lines (7 and 9) — no drift.
    expect(blocks).toEqual([
      { tag: 'h1', start: '1', end: '1' },
      { tag: 'p', start: '3', end: '3' },
      { tag: 'img', start: '5', end: '5' },
      { tag: 'img', start: '5', end: '5' },
      { tag: 'p', start: '7', end: '7' },
      { tag: 'p', start: '9', end: '9' },
    ]);
  });
});

/**
 * The pipeline is the ONLY stamp source. The sanitizer strips any user-supplied
 * `data-line-*`, and every real stamp is written downstream of it. A raw element
 * carrying crafted stamps therefore cannot forge provenance — it is re-stamped
 * from its true source position (or left per the totality rules).
 */
describe('provenance stamps are pipeline-owned (raw stamps cannot be forged)', () => {
  test('a raw div + raw wiki-link with crafted stamps are re-stamped from true lines', () => {
    const source = [
      '# Real Title', // 1
      '', // 2
      'Real paragraph.', // 3
      '', // 4
      // Forgery: raw HTML block claiming the line-1 heading's range, plus a raw
      // wiki-link also wearing a crafted stamp.
      '<div data-line-start="1" data-line-end="1">forged<wiki-link data-line-start="1" data-line-end="1" data-target="x">w</wiki-link></div>', // 5
      '', // 6
      '$$', // 7
      'E = mc^2', // 8
      '$$', // 9
    ].join('\n');

    const doc = parseMarkdown(source);
    const blocks = topBlocks(doc.hast);

    // The forged div is stamped from its OWN true source line (5), never the
    // line-1 value it tried to smuggle: the sanitizer stripped the raw
    // data-line-*, and the pipeline re-stamped from position.
    const forgedDiv = blocks.find((b) => b.tag === 'div' && b.start === '5');
    expect(forgedDiv).toEqual({ tag: 'div', start: '5', end: '5' });

    // Nothing wears the forged (1,1) range except the genuine line-1 heading.
    const wearingForged = blocks.filter((b) => b.start === '1' && b.end === '1');
    expect(wearingForged).toEqual([{ tag: 'h1', start: '1', end: '1' }]);

    // No surviving element anywhere in the tree carries a raw data-line stamp
    // other than the pipeline's own top-level ones (the smuggled wiki-link stamp
    // is gone).
    let wikiStamped = false;
    visit(doc.hast, 'element', (node: Element) => {
      if (node.tagName === 'wiki-link') {
        wikiStamped =
          node.properties?.dataLineStart != null || node.properties?.dataLineEnd != null;
      }
    });
    expect(wikiStamped).toBe(false);

    // The real block math is still stamped from its own lines (7-9) and katex
    // still rendered inside the wrapper (the stamp move didn't break math).
    const mathDiv = blocks.find((b) => b.tag === 'div' && b.start === '7');
    expect(mathDiv).toEqual({ tag: 'div', start: '7', end: '9' });
    let katexRendered = false;
    visit(doc.hast, 'element', (node: Element) => {
      const className = node.properties?.className;
      if (Array.isArray(className) && className.some((c) => String(c).startsWith('katex'))) {
        katexRendered = true;
      }
    });
    expect(katexRendered).toBe(true);
  });
});

/**
 * Provenance is transferred BY NODE, never by ordering. These are the reviewer's
 * attacks against the old class-matched FIFO queue: a positionless (or merely
 * katex-classed) raw element must not be able to claim a real block-math range.
 */
describe('block-math provenance is origin-faithful (no ordering queue)', () => {
  test('a katex-classed raw <wiki-link> before block math cannot steal its range', () => {
    // The reviewer's exact attack: a raw <wiki-link> wearing a math/katex class,
    // placed BEFORE the real block math. Under the old class-matched FIFO an
    // origin-blind katex-classed node could shift the math queue and claim the
    // math's line range. There is no queue any more, and the smuggled class is
    // stripped by the sanitizer, so neither vector exists.
    const source = [
      '# Title', // 1
      '', // 2
      '<wiki-link class="math-display katex-display">x</wiki-link>', // 3 (attack)
      '', // 4
      '$$', // 5
      'E = mc^2', // 6
      '$$', // 7
    ].join('\n');

    const doc = parseMarkdown(source);
    const blocks = topBlocks(doc.hast);

    // The one real block math is stamped from ITS OWN source lines (5-7), via the
    // pipeline-owned wrapper <div>.
    const mathDivs = blocks.filter((block) => block.tag === 'div');
    expect(mathDivs).toEqual([{ tag: 'div', start: '5', end: '7' }]);

    // Nothing OTHER than that wrapper carries the math's range — no theft.
    const thieves = blocks.filter(
      (block) => block.tag !== 'div' && block.start === '5' && block.end === '7',
    );
    expect(thieves).toEqual([]);

    // The enabling hole is closed: the smuggled math/katex class is stripped, so
    // a <wiki-link> can never masquerade as block math (only `wikilink` is kept).
    let wikiClassName: unknown;
    visit(doc.hast, 'element', (node: Element) => {
      if (node.tagName === 'wiki-link') wikiClassName = node.properties?.className;
    });
    const className = Array.isArray(wikiClassName) ? wikiClassName : [];
    expect(className).not.toContain('math-display');
    expect(className).not.toContain('katex-display');
  });

  test('math interleaved with raw HTML: every block keeps its own source lines', () => {
    const source = [
      '# H', // 1
      '', // 2
      '<blockquote>alpha</blockquote>', // 3  raw HTML block
      '', // 4
      '$$', // 5
      'a^2', // 6
      '$$', // 7
      '', // 8
      '<blockquote>beta</blockquote>', // 9  raw HTML block
      '', // 10
      '$$', // 11
      'b^2', // 12
      '$$', // 13
    ].join('\n');

    const blocks = topBlocks(parseMarkdown(source).hast);

    // Raw HTML blocks are stamped from their own positions (rehype-raw), block
    // math from the wrapper <div> — each independent, none shifted by a neighbour.
    expect(blocks).toEqual([
      { tag: 'h1', start: '1', end: '1' },
      { tag: 'blockquote', start: '3', end: '3' },
      { tag: 'div', start: '5', end: '7' }, // first block math
      { tag: 'blockquote', start: '9', end: '9' },
      { tag: 'div', start: '11', end: '13' }, // second block math
    ]);
  });
});
