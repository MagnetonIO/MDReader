import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { toHtml } from 'hast-util-to-html';
import { describe, expect, test } from 'vitest';

import { parseMarkdown } from './pipeline.js';
import { parityCases } from './parity-fixtures.js';
import { readCorpus } from './test-utils.js';

/**
 * DOM ↔ worker OUTPUT PARITY (genuine cross-variant regression guard).
 *
 * `apps/mdreader/ui/vite.config.ts` prepends the DOM-free `worker` export
 * condition app-wide, so the WORKER bundle (big docs) and the MAIN-THREAD/client
 * bundle (small docs, plus the webview itself) both resolve the `worker`
 * variants of the pipeline's document-touching deps instead of their `browser`
 * variants. This guard proves that switch is invisible: the `browser`/DOM
 * variants and the `worker` variants must serialize byte-identically.
 *
 * The SAME file runs under TWO vitest projects that resolve DIFFERENT variants
 * against the SAME committed reference (`__fixtures__/pipeline-output.reference.json`,
 * produced by the committed generator `gen-parity-reference.ts` via
 * `pnpm --filter @mdreader/markdown gen:parity-reference`):
 *
 *  - `dom`    — `browser` conditions + a REAL injected `document`/`DOMParser`
 *               (`dom-globals.setup.ts`), so `decode-named-character-reference`
 *               runs its `index.dom.js` decoder (`element.innerHTML` on a real
 *               jsdom node) and `hast-util-from-html-isomorphic` runs `browser.js`
 *               (the genuine DOM path the webview used before the fix). Without
 *               the injected DOM this project throws — proof the DOM variants are
 *               actually the ones loaded.
 *  - `worker` — the `worker` export condition ahead of `browser`, no `document`
 *               (the real Web Worker path) → the DOM-free JS-table decoder /
 *               parse5 variants.
 *
 * The cases (see `parity-fixtures.ts`) cover KaTeX AND named/decimal/hex/astral
 * entity references in prose and code, so BOTH document-touching deps are
 * exercised across variants. Both projects asserting equality to the one
 * committed reference ⇒ DOM output == worker output for every case. If any dep's
 * `worker` export ever diverges in OUTPUT from its `browser` export, the two
 * projects disagree with the reference and this guard fails — which is also the
 * main-thread safety net for the app-wide condition change (item e).
 */

const reference: Record<string, string> = JSON.parse(
  readFileSync(fileURLToPath(new URL('./__fixtures__/pipeline-output.reference.json', import.meta.url)), 'utf8'),
) as Record<string, string>;

describe('DOM ↔ worker output parity', () => {
  test.each(parityCases(readCorpus))('$name serializes identically across resolution variants', ({ name, source }) => {
    const expected = reference[name];
    // A missing reference entry is a hard failure, never a silent skip.
    expect(typeof expected).toBe('string');

    const html = toHtml(parseMarkdown(source).hast);
    expect(html).toBe(expected);
  });
});
