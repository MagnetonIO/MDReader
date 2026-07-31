import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { toHtml } from 'hast-util-to-html';
import { test } from 'vitest';

import { parseMarkdown } from './pipeline.js';
import { parityCases } from './parity-fixtures.js';
import { readCorpus } from './test-utils.js';

/**
 * Committed regenerator for the DOM/worker parity reference.
 *
 * Run with `pnpm --filter @mdreader/markdown gen:parity-reference` (which points
 * vitest at THIS file via `vitest.gen.config.ts`, under default Node resolution).
 * It writes `__fixtures__/pipeline-output.reference.json` — the byte-for-byte
 * serialized-HTML contract that BOTH the `dom` (browser variants + real document)
 * and `worker` (DOM-free) projects of `dom-worker-parity.test.ts` assert against.
 *
 * It is NOT a `*.test.ts` file, so the normal suite never runs it; regenerate
 * deliberately whenever `parity-fixtures.ts` or the rendering contract changes.
 */
test('regenerate the DOM/worker parity reference', () => {
  const output: Record<string, string> = {};
  for (const { name, source } of parityCases(readCorpus)) {
    output[name] = toHtml(parseMarkdown(source).hast);
  }
  const dest = fileURLToPath(new URL('./__fixtures__/pipeline-output.reference.json', import.meta.url));
  writeFileSync(dest, `${JSON.stringify(output, null, 2)}\n`, 'utf8');
});
