import { describe, expect, test } from 'vitest';

import { parseMarkdown } from './pipeline.js';
import { parseFrontmatterBlock } from './frontmatter.js';
import { isFrontmatterError } from './types.js';
import { readCorpus } from './test-utils.js';

describe('frontmatter extraction', () => {
  test('typical frontmatter parses into structured data', () => {
    const doc = parseMarkdown(readCorpus('frontmatter/typical.md'));
    expect(isFrontmatterError(doc.frontmatter)).toBe(false);
    expect(doc.frontmatter).toMatchObject({
      title: 'Typical Frontmatter',
      author: 'M. Long',
      published: true,
      rating: 4.5,
      tags: ['example', 'frontmatter'],
    });
  });

  test('malformed frontmatter is tolerant: {raw, error}, body still renders', () => {
    const doc = parseMarkdown(readCorpus('frontmatter/malformed-frontmatter.md'));
    expect(isFrontmatterError(doc.frontmatter)).toBe(true);
    if (isFrontmatterError(doc.frontmatter)) {
      expect(doc.frontmatter.raw).toContain('title: Malformed');
      expect(doc.frontmatter.error).toBeTruthy();
    }
    // The body heading is still present.
    expect(doc.outline[0]?.text).toBe('Malformed Frontmatter');
    expect(doc.stats.blockCount).toBeGreaterThan(0);
  });

  test('empty frontmatter is treated as no metadata (null)', () => {
    const doc = parseMarkdown(readCorpus('frontmatter/empty-frontmatter.md'));
    expect(doc.frontmatter).toBeNull();
    expect(doc.outline[0]?.text).toBe('Empty Frontmatter');
  });

  test('frontmatter-only document parses metadata with an empty body', () => {
    const doc = parseMarkdown(readCorpus('frontmatter/frontmatter-only.md'));
    expect(doc.frontmatter).toMatchObject({ title: 'Frontmatter Only', body: false });
    expect(doc.stats.blockCount).toBe(0);
  });

  test('a document with no frontmatter yields null', () => {
    const doc = parseMarkdown('# Just a heading\n\nBody.\n');
    expect(doc.frontmatter).toBeNull();
  });

  test('parseFrontmatterBlock never throws on garbage', () => {
    const result = parseFrontmatterBlock(': : : [unclosed\n\tbad');
    expect(isFrontmatterError(result)).toBe(true);
  });
});
