/**
 * Inputs for the DOM/worker output-parity guard (`dom-worker-parity.test.ts`)
 * and its committed-reference generator (`gen-parity-reference.ts`). Chosen to
 * exercise BOTH document-touching dependencies across their DOM (`browser`) and
 * DOM-free (`worker`) export variants:
 *
 *  - `hast-util-from-html-isomorphic` — via KaTeX (`katex-*.md` corpus fixtures);
 *  - `decode-named-character-reference` — the dep whose `browser`/`index.dom.js`
 *    variant (`document.createElement('i')` + `element.innerHTML`) caused the
 *    crash. Its decoder only runs for NAMED entity references in text/flow, so
 *    the inline cases below feed named entities (`&amp;`, `&copy;`, `&mdash;`,
 *    the `&notin;`/`&notit;` semicolon-completeness edge) alongside decimal
 *    (`&#8212;`) and hex/astral (`&#x1F600;`) numeric references, in prose AND in
 *    code/inline-code contexts — so the DOM decoder (real jsdom) and the JS table
 *    decoder must agree byte-for-byte.
 */
export const PARITY_CORPUS_FIXTURES = [
  'docs/guides/gfm-tables.md',
  'docs/guides/task-lists.md',
  'docs/guides/footnotes.md',
  'docs/guides/strikethrough-autolinks.md',
  'docs/guides/code-fences.md',
  'edge-cases/unclosed-fence.md',
  'docs/guides/mermaid-flowchart.md',
  'docs/guides/katex-inline.md',
  'docs/guides/katex-block.md',
  'notes/zettel/wikilinks.md',
  'notes/zettel/broken-links.md',
  'frontmatter/typical.md',
  'frontmatter/malformed-frontmatter.md',
  'docs/reference/headings-with-inline.md',
  'edge-cases/unicode-emoji-cjk.md',
] as const;

/**
 * Inline entity-reference cases (named / decimal / hex / astral) — the crashing
 * dep's decode path, in prose and code/text contexts.
 */
export const PARITY_ENTITY_CASES: readonly { readonly name: string; readonly source: string }[] = [
  {
    name: 'entities/named-prose',
    source: 'Ben &amp; Jerry &copy; 2026 &mdash; a dash, &hellip; and &trade;. Non&nbsp;breaking space.\n',
  },
  {
    name: 'entities/named-edge-cases',
    // `&notin;` -> `∉` (full), `&notit;` -> `¬it;` (partial `&not` match, no
    // closing semicolon consumed) — the exact branch index.dom.js hand-rolls.
    source: 'Set: a &notin; B, but &notit; is not-it. Also &lt;tag&gt; and &quot;q&quot; &apos;a&apos;.\n',
  },
  {
    name: 'entities/numeric-decimal-hex-astral',
    source: 'Decimal &#8212; em dash, hex &#x2014; em dash, astral &#x1F600; grin, &#128512; grin again.\n',
  },
  {
    name: 'entities/in-code-contexts',
    // In code, markdown does NOT entity-decode; `&` is HTML-escaped on output.
    // Included so parity covers the escaped path too, next to decoded prose.
    source: 'Inline `&amp; &copy; &#x1F600;` code, then a block:\n\n```\n&amp; &mdash; &#8212; &#x1F600;\n```\n\nAnd prose &amp; after.\n',
  },
  {
    name: 'entities/in-headings-and-links',
    source: '# Heading &amp; &copy; &mdash;\n\nA [link &amp; text](https://example.test/?a=1&amp;b=2) and *emph &trade;*.\n',
  },
] as const;

/**
 * The full parity case set as `{ name, source }`, resolving corpus fixtures via
 * the caller-supplied reader (kept out of this module so it stays fs-free and
 * excluded from the build). Used identically by the parity test and the
 * reference generator so the two can never disagree on inputs.
 */
export function parityCases(readCorpus: (relative: string) => string): { name: string; source: string }[] {
  return [
    ...PARITY_CORPUS_FIXTURES.map((name) => ({ name, source: readCorpus(name) })),
    ...PARITY_ENTITY_CASES.map((entityCase) => ({ name: entityCase.name, source: entityCase.source })),
  ];
}
