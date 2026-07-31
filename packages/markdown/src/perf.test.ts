import { describe, expect, test } from 'vitest';

import { parseMarkdown } from './pipeline.js';
import { shouldVirtualize, VIRTUALIZATION_BLOCK_THRESHOLD } from './thresholds.js';
import { readCorpus } from './test-utils.js';

describe('performance smoke', () => {
  test('parses a >1MB document synchronously in under 3s', () => {
    const source = readCorpus('edge-cases/big-file.md');
    expect(source.length).toBeGreaterThan(1_000_000);

    const start = performance.now();
    const doc = parseMarkdown(source);
    const elapsedMs = performance.now() - start;

    expect(doc.stats.byteLength).toBeGreaterThan(1_000_000);
    expect(elapsedMs).toBeLessThan(3000);
  });

  test('a big document trips the virtualization threshold', () => {
    const doc = parseMarkdown(readCorpus('edge-cases/big-file.md'));
    expect(doc.stats.blockCount).toBeGreaterThan(VIRTUALIZATION_BLOCK_THRESHOLD);
    expect(shouldVirtualize(doc.stats)).toBe(true);
  });
});
