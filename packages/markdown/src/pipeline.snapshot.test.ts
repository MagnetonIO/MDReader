import { toHtml } from 'hast-util-to-html';
import { describe, expect, test } from 'vitest';

import { parseMarkdown } from './pipeline.js';
import { readCorpus } from './test-utils.js';

/**
 * hast snapshot coverage across the corpus. Each fixture exercises a distinct
 * slice of the pipeline; the serialized HTML is the rendering contract shared
 * verbatim by the app and the website. KaTeX markup is deterministic for the
 * pinned katex version.
 */
const FIXTURES = [
  'docs/guides/gfm-tables.md', // GFM table
  'docs/guides/task-lists.md', // GFM task list
  'docs/guides/footnotes.md', // GFM footnotes (hoisted section)
  'docs/guides/strikethrough-autolinks.md', // GFM strikethrough + autolink
  'docs/guides/code-fences.md', // fenced + tilde + indented code
  'edge-cases/unclosed-fence.md', // unclosed fence to EOF
  'docs/guides/mermaid-flowchart.md', // mermaid fences preserved
  'docs/guides/katex-inline.md', // inline KaTeX
  'docs/guides/katex-block.md', // block + inline KaTeX
  'notes/zettel/wikilinks.md', // wikilinks: plain/heading/label/heading+label
  'notes/zettel/broken-links.md', // broken wikilinks/links still render
  'frontmatter/typical.md', // typical frontmatter (body render)
  'frontmatter/malformed-frontmatter.md', // malformed frontmatter (body still renders)
  'frontmatter/empty-frontmatter.md', // empty frontmatter
  'docs/reference/no-headings.md', // no headings
  'edge-cases/crlf-endings.md', // CRLF line endings
  'edge-cases/unicode-emoji-cjk.md', // unicode / emoji / CJK
] as const;

describe('parseMarkdown hast snapshots', () => {
  test.each(FIXTURES)('%s renders stable hast', (fixture) => {
    const doc = parseMarkdown(readCorpus(fixture));
    expect(toHtml(doc.hast)).toMatchSnapshot();
  });
});
