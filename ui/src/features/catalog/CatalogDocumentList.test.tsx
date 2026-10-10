import { cleanup, render, screen, act } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { CatalogDocumentList } from './CatalogDocumentList';
import type { CatalogDocument, CatalogPage } from './backend';
afterEach(cleanup);
const source: CatalogPage['source'] = { catalogId: 'remote:selected', name: 'Notes', kind: 'dropbox', enabled: true,
  librarySourceId: null, failure: null, coverage: 'selected_metadata', observedAtMs: 1 };
const file = (path: string, revision: string): CatalogDocument => ({ name: 'README.md', displayPath: path,
  observation: 'cached_provider_revision', modifiedAtMs: 1, sizeBytes: null, observedAtMs: 1,
  open: { kind: 'exact', sourceId: `provider:${'a'.repeat(64)}`, path, revision } });
test('native reordering/revision replacement retains the focused exact file, not its title or array index', () => {
  const first = file('first.md', 'b'.repeat(64)); const second = file('second.md', 'c'.repeat(64));
  const props = { busy: false, onOpen: vi.fn(), onMore: vi.fn() };
  const page: CatalogPage = { operation: 'page', source, documents: [first, second], nextCursor: null };
  const view = render(<CatalogDocumentList {...props} page={page} />);
  const button = screen.getByRole('button', { name: 'Open README.md — second.md' });
  act(() => button.focus());
  view.rerender(<CatalogDocumentList {...props} page={{ ...page, documents: [file('second.md', 'd'.repeat(64)), first] }} />);
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Open README.md — second.md' }));
  expect(document.activeElement).toBe(button);
  expect(screen.getAllByRole('button').map(node => node.getAttribute('aria-label'))).toEqual(['Open README.md — second.md', 'Open README.md — first.md']);
});
