import { expect, test } from 'vitest';
import { parseMarkdown } from '@mdreader/markdown';
import { fragmentTarget } from './fragment-navigation';
function blocks(markdown: string) { return parseMarkdown(markdown).hast.children.filter((node) => node.type === 'element'); }
test('uses real final IDs for repeated headings, footnotes, backlinks and literal percent IDs', () => {
  const content = blocks('## Same\n\n## Same\n\nNote[^a].\n\n[^a]: Definition.\n\n<a id="literal%20"></a>');
  expect(fragmentTarget(content, 'same')?.block).toBe(0);
  expect(fragmentTarget(content, 'same-1')?.block).toBe(1);
  expect(fragmentTarget(content, 'user-content-fnref-a')?.block).toBe(2);
  expect(fragmentTarget(content, 'user-content-fn-a')?.block).toBe(content.length - 1);
  expect(fragmentTarget(content, 'literal%20')?.id).toBe('user-content-literal%20');
  expect(fragmentTarget(content, 'literal ')).toBeNull();
  expect(fragmentTarget(content, 'absent')).toBeNull();
});
