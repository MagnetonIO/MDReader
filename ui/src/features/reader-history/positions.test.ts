import { expect, test } from 'vitest';
import { parseMarkdown } from '@mdreader/markdown';
import { restoreAnchor, savedAnchor } from './positions';

test('positions bind actual parsed source lines to the verified revision and reject stale/out-of-body anchors', () => {
  const doc = parseMarkdown('# Heading\n\nParagraph.\n\n## Next');
  const revision = 'a'.repeat(64);
  const position = savedAnchor(doc, revision, { block: 1, offset: 12 });
  expect(position).toEqual({ revision, line: 3, block: 1, offset: 12 });
  expect(restoreAnchor(doc, revision, 5, position).anchor).toEqual({ block: 1, offset: 12 });
  expect(restoreAnchor(doc, 'b'.repeat(64), 5, position)).toMatchObject({ anchor: null, line: null, notice: expect.stringContaining('unavailable') });
  expect(restoreAnchor(doc, revision, 2, position).anchor).toBeNull();
  expect(restoreAnchor(doc, revision, 5, { ...position!, block: 99 })).toMatchObject({ anchor: null, line: 3, notice: expect.stringContaining('layout changed') });
});
