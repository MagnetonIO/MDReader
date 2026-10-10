import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { DocumentDetails } from './DocumentDetails';
import type { DocumentLocation } from './document-location';
const reference = { sourceId: 'a'.repeat(64), revision: 'b'.repeat(64), path: 'notes/index.md' };
afterEach(cleanup);
test('shows verified local path and friendly source with raw identifiers inside collapsed technical details', async () => {
  const resolver = vi.fn(async () => ({ file: 'index.md', source: 'Public folder', path: '/Public folder/notes/index.md' }));
  const view = render(<DocumentDetails reference={reference} kind="local" verified resolver={resolver} />);
  await screen.findByText('/Public folder/notes/index.md');
  expect(screen.getByText('Public folder')).toBeTruthy();
  const technical = screen.getByText('Technical details').closest('details')!;
  expect(technical.open).toBe(false);
  expect(screen.getByText(reference.sourceId).closest('details')).toBe(technical);
  expect(screen.getByText(reference.revision).closest('details')).toBe(technical);
  expect(view.container.querySelector('p')).toBeNull();
  fireEvent.click(screen.getByText('Technical details')); expect(technical.open).toBe(true);
});
test('old location cannot publish during a selection switch or while the current body is unverified', async () => {
  let finish!: (location: DocumentLocation) => void;
  const resolver = vi.fn(() => new Promise<DocumentLocation>(resolve => { finish = resolve; }));
  const view = render(<DocumentDetails reference={reference} kind="local" verified resolver={resolver} />);
  view.rerender(<DocumentDetails reference={{ ...reference, path: 'new.md' }} kind="local" verified={false} resolver={resolver} />);
  await act(async () => finish({ file: 'index.md', source: 'Old folder', path: '/Old folder/index.md' }));
  expect(screen.queryByText('/Old folder/index.md')).toBeNull(); expect(screen.queryByText('Full path')).toBeNull();
  expect(resolver).toHaveBeenCalledOnce();
});
test('unknown cloud location never presents the cache record key as an original filename or disk path', async () => {
  const path = `records/${'c'.repeat(64)}.md`;
  render(<DocumentDetails reference={{ ...reference, sourceId: `provider:${reference.sourceId}`, path, title: path }} kind="provider" verified resolver={async () => null} />);
  expect(screen.getByText('Markdown document')).toBeTruthy();
  expect(screen.queryByText(path)).toBeNull(); expect(screen.queryByText('Full path')).toBeNull();
});
