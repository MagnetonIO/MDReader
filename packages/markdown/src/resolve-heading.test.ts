import { expect, test } from 'vitest';
import { parseMarkdown } from './pipeline.js';
import { resolveHeading } from './resolve-heading.js';
test('resolves actual sanitized IDs, bare fragments and unique heading text to source lines', () => {
  const doc = parseMarkdown('# Intro\n\n## Section two\n');
  for (const target of [doc.outline[1]!.slug, 'section-two', 'SECTION TWO']) {
    expect(resolveHeading(doc.outline, target)?.lineStart).toBe(3);
  }
  expect(resolveHeading(doc.outline, 'missing')).toBeNull();
});
test('duplicate slug suffixes follow renderer IDs and ambiguous text does not guess', () => {
  const doc = parseMarkdown('## Same\n\n## Same\n');
  expect(resolveHeading(doc.outline, doc.outline[1]!.slug)?.lineStart).toBe(3);
  expect(resolveHeading(doc.outline, 'same')?.lineStart).toBe(1);
  expect(resolveHeading(doc.outline, 'SAME')).toBeNull();
});
