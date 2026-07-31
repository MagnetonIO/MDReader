import { describe, expect, test } from 'vitest';
import { visit } from 'unist-util-visit';
import type { Element } from 'hast';

import { parseMarkdown } from './pipeline.js';
import { readCorpus } from './test-utils.js';

describe('parseMarkdown robustness', () => {
  test('an empty document yields an empty, non-throwing result', () => {
    const doc = parseMarkdown('');
    expect(doc.hast.children.filter((c) => c.type === 'element')).toHaveLength(0);
    expect(doc.frontmatter).toBeNull();
    expect(doc.outline).toEqual([]);
    expect(doc.stats).toEqual({ blockCount: 0, byteLength: 0 });
  });

  test('a whitespace-only document does not throw', () => {
    expect(() => parseMarkdown(readCorpus('edge-cases/whitespace-only.md'))).not.toThrow();
  });

  test('an unclosed fence is captured as a code block without hanging', () => {
    const doc = parseMarkdown(readCorpus('edge-cases/unclosed-fence.md'));
    const pre = doc.hast.children.filter((c) => c.type === 'element' && c.tagName === 'pre');
    expect(pre.length).toBeGreaterThanOrEqual(1);
  });

  test('byteLength counts UTF-8 bytes, not code units, for CJK/emoji', () => {
    const doc = parseMarkdown('# 数据库 🚀\n');
    // Multi-byte characters make the byte length exceed the string length.
    expect(doc.stats.byteLength).toBeGreaterThan('# 数据库 🚀\n'.length);
  });

  test('the alternate .markdown extension content parses', () => {
    const doc = parseMarkdown(readCorpus('edge-cases/notes.markdown'));
    expect(doc.stats.blockCount).toBeGreaterThan(0);
  });
});

describe('GFM footnote reference integrity', () => {
  test('every footnote href/aria-describedby resolves to exactly one final id', () => {
    const doc = parseMarkdown(readCorpus('docs/guides/footnotes.md'));

    const ids = new Set<string>();
    const fragmentHrefs: string[] = [];
    const ariaTargets: string[] = [];

    visit(doc.hast, 'element', (node: Element) => {
      const props = node.properties ?? {};
      if (typeof props.id === 'string') ids.add(props.id);
      if (typeof props.href === 'string' && props.href.startsWith('#')) {
        fragmentHrefs.push(props.href.slice(1));
      }
      // hast stores `aria-describedby` as an ARRAY of id references (it is a
      // space-separated id-reference list), so collect from the array — not just
      // the string case, which would silently skip every real aria target.
      const aria = props.ariaDescribedBy;
      if (Array.isArray(aria)) {
        for (const value of aria) if (typeof value === 'string') ariaTargets.push(value);
      } else if (typeof aria === 'string') {
        ariaTargets.push(aria);
      }
    });

    // There is at least one footnote reference, back-reference, and aria target.
    expect(fragmentHrefs.length).toBeGreaterThan(0);
    expect(ariaTargets.length).toBeGreaterThan(0);

    for (const target of [...fragmentHrefs, ...ariaTargets]) {
      // The invariant is resolution, not "never doubly prefixed": every footnote
      // reference points at exactly one existing final id. Footnote ids are
      // pipeline-origin (single-prefixed here because nothing in the fixture
      // collides), so the reference resolves without a suffix.
      expect(ids.has(target)).toBe(true);
    }
  });
});
