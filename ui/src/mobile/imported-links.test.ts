import { expect, test } from 'vitest';
import type { DeviceDocument } from '../bindings';
import { resolveImportedLink } from './imported-links';
const document: DeviceDocument = { id: 'selected-id', name: 'Two words.md', source_id: 'selected-source', revision: 'a'.repeat(64), bytes: 20, pinned: false, line: 1, indexed: true };
test('resolves selected flat filenames and wiki extension aliases without losing identity or fragment', () => {
  expect(resolveImportedLink([document], 'relative', './Two%20words.md#Section%20two')).toEqual({ state: 'resolved', document, heading: 'Section two' });
  expect(resolveImportedLink([document], 'wikilink', 'Two words')).toEqual({ state: 'resolved', document, heading: null });
  expect(resolveImportedLink([{ ...document, name: 'literal%2Fname.md' }], 'relative', 'literal%252Fname.md')).toMatchObject({ state: 'resolved' });
});
test('duplicate selected names are ambiguous and missing files never infer a source', () => {
  expect(resolveImportedLink([document, { ...document, id: 'another-selected-id' }], 'relative', document.name)).toEqual({ state: 'ambiguous' });
  expect(resolveImportedLink([document], 'wikilink', 'Other')).toEqual({ state: 'missing' });
});
test('qualified paths and malformed targets cannot fall back to a selected basename', () => {
  for (const target of ['../Two words.md', 'folder/Two words.md', '%2e%2e%2fTwo words.md', '//host/Two words.md']) {
    expect(resolveImportedLink([document], 'relative', target)).toEqual({ state: 'unsupported' });
  }
  for (const target of ['https%3A//host/Two words.md', 'Two%00words.md', 'Two words.md#%0a', '%ZZ.md']) {
    expect(resolveImportedLink([document], 'relative', target)).toEqual({ state: 'invalid' });
  }
});
