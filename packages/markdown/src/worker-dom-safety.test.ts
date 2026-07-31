import { describe, expect, test } from 'vitest';

import { parseMarkdown } from './pipeline.js';
import { WORKER_BYTE_THRESHOLD } from './thresholds.js';

/**
 * WORKER PARSE PATH — no-`document` safety (regression guard).
 *
 * This file runs ONLY in the `worker` vitest project (see `vitest.config.ts`),
 * whose module resolution mirrors the real Web Worker bundle: the pipeline's
 * transitive dependencies are resolved with the `worker` export condition ahead
 * of `browser` (from {@link WORKER_BUNDLE_RESOLVE_CONDITIONS}), under a Node
 * environment that — exactly like a dedicated worker — has NO `document` /
 * `window`. Documents at or above {@link WORKER_BYTE_THRESHOLD} (64 KiB) parse
 * off-thread through this exact path.
 *
 * Why the rest of the suite missed the bug: every other test resolves the
 * DOM-free variants by default (Node conditions), so `document` being absent is
 * never actually exercised against the `browser` variants the worker bundle
 * ships. Reverting the fix — dropping `'worker'` from
 * `WORKER_BUNDLE_RESOLVE_CONDITIONS` — makes `decode-named-character-reference`
 * and `hast-util-from-html-isomorphic` resolve their `browser` variants, whose
 * top-level `document.createElement(...)` throws `ReferenceError: document is not
 * defined` the instant this file imports `./pipeline.js`, turning the whole file
 * red at collection time.
 */

/** A source of at least `bytes` UTF-8 bytes that exercises the two DOM-touching
 * deps: HTML entities (decode-named-character-reference) and, optionally, KaTeX
 * math (hast-util-from-html-isomorphic). */
function docOfAtLeast(bytes: number, withMath: boolean): string {
  const filler = 'The quick brown fox jumps over the lazy dog. ';
  let out = '# Worker path\n\nEntities: &amp; &copy; &notin; &#x2603; &notit;.\n\n';
  if (withMath) {
    out += 'Inline $E = mc^2$ and a block:\n\n$$\\int_0^1 x^2\\,dx = \\tfrac13$$\n\n';
  }
  while (new TextEncoder().encode(out).length < bytes) out += `${filler}\n\n`;
  return out;
}

describe('worker parse path is DOM-free', () => {
  test('runs in a worker-like scope with no document/window', () => {
    // If this fails, the `worker` project is no longer running under a real
    // no-DOM (Node) environment and the guard below is toothless.
    expect(typeof (globalThis as { document?: unknown }).document).toBe('undefined');
    expect(typeof (globalThis as { window?: unknown }).window).toBe('undefined');
  });

  test('parses a >64 KiB document with no document global', () => {
    const source = docOfAtLeast(WORKER_BYTE_THRESHOLD + 1024, false);
    expect(new TextEncoder().encode(source).length).toBeGreaterThan(WORKER_BYTE_THRESHOLD);

    let doc!: ReturnType<typeof parseMarkdown>;
    expect(() => {
      doc = parseMarkdown(source);
    }).not.toThrow();

    expect(doc.stats.byteLength).toBeGreaterThan(WORKER_BYTE_THRESHOLD);
    expect(doc.stats.blockCount).toBeGreaterThan(0);
  });

  test('parses a >64 KiB document containing KaTeX with no document global', () => {
    const source = docOfAtLeast(WORKER_BYTE_THRESHOLD + 1024, true);

    let doc!: ReturnType<typeof parseMarkdown>;
    expect(() => {
      doc = parseMarkdown(source);
    }).not.toThrow();

    // KaTeX was rendered to hast entirely without a DOM: its output markup is
    // present in the tree.
    expect(JSON.stringify(doc.hast)).toContain('katex');
  });
});
