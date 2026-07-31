import { parseMarkdown } from '@mdreader/markdown';
import type { ReactNode } from 'react';

import { MarkdownReader } from '../markdown-reader.js';
import { Outline } from '../outline.js';

// Real corpus files, read verbatim via Vite/Ladle `?raw`. These are the same
// fixtures the tests parse, so stories and tests never drift.
import typicalDoc from '../../../../fixtures/corpus/frontmatter/typical.md?raw';
import gfmTables from '../../../../fixtures/corpus/docs/guides/gfm-tables.md?raw';
import taskLists from '../../../../fixtures/corpus/docs/guides/task-lists.md?raw';
import footnotes from '../../../../fixtures/corpus/docs/guides/footnotes.md?raw';
import strikethrough from '../../../../fixtures/corpus/docs/guides/strikethrough-autolinks.md?raw';
import katexBlock from '../../../../fixtures/corpus/docs/guides/katex-block.md?raw';
import mermaidFlow from '../../../../fixtures/corpus/docs/guides/mermaid-flowchart.md?raw';
import wikilinks from '../../../../fixtures/corpus/notes/zettel/wikilinks.md?raw';
import bigFile from '../../../../fixtures/corpus/edge-cases/big-file.md?raw';

export default {
  title: 'Reader/MarkdownReader',
};

function Frame({ children }: { children: ReactNode }) {
  return <div style={{ height: '90vh', border: '1px solid var(--color-border)' }}>{children}</div>;
}

/** A small document with frontmatter, headings, and a couple of links. */
export function SmallDoc() {
  return (
    <Frame>
      <MarkdownReader doc={parseMarkdown(typicalDoc)} />
    </Frame>
  );
}

/** GFM torture: tables, task lists, footnotes, strikethrough, and autolinks. */
export function GfmTorture() {
  const source = [gfmTables, taskLists, footnotes, strikethrough].join('\n\n---\n\n');
  return (
    <Frame>
      <MarkdownReader doc={parseMarkdown(source)} />
    </Frame>
  );
}

/** Math (KaTeX block + inline) alongside Mermaid diagrams and wikilinks. */
export function MathMermaidAndWikilinks() {
  const source = [katexBlock, mermaidFlow, wikilinks].join('\n\n---\n\n');
  return (
    <Frame>
      <MarkdownReader doc={parseMarkdown(source)} />
    </Frame>
  );
}

/** A >1MB document, rendered through the virtualization path. */
export function HugeDoc() {
  return (
    <Frame>
      <MarkdownReader doc={parseMarkdown(bigFile)} />
    </Frame>
  );
}

/** The reader with its right-rail outline alongside. */
export function WithOutline() {
  const doc = parseMarkdown([katexBlock, gfmTables, footnotes].join('\n\n---\n\n'));
  return (
    <Frame>
      <div style={{ display: 'grid', gridTemplateColumns: '1fr 16rem', height: '100%' }}>
        <MarkdownReader doc={doc} />
        <Outline items={doc.outline} activeSlug={doc.outline[1]?.slug} />
      </div>
    </Frame>
  );
}
