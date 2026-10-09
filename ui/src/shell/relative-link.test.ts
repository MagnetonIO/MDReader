import { expect, test } from 'vitest';
import { resolveWorkspaceLink } from './relative-link';
const files = ['notes/Next note.md', 'notes/literal%20.md', 'README.markdown'].map((name) => ({
  path: `/public/${name}`, name: name.split('/').at(-1)!, dir: '/public', mtime: 0, size: 1, ignored: false,
}));
test('nested relative paths and encoded filenames preserve exactly decoded cross-document anchors', () => {
  expect(resolveWorkspaceLink(files, '/public/notes/deep', '../Next%20note.md#section%20two'))
    .toEqual({ file: files[0], fragment: 'section two' });
  expect(resolveWorkspaceLink(files, '/public/notes', 'literal%2520.md#literal%2520'))
    .toEqual({ file: files[1], fragment: 'literal%20' });
  expect(resolveWorkspaceLink(files, '/public/notes', '../README.markdown')).toEqual({ file: files[2], fragment: null });
});
test.each(['../../private.md', '%ZZ.md', '%2Fprivate.md', 'file:///private.md', 'missing.md'])('never navigates an unlisted or malformed target %s', (href) => {
  expect(resolveWorkspaceLink(files, '/public/notes', href)).toBeNull();
});
