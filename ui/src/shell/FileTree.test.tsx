import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type { WorkspaceFile } from '../platform/fs.js';
import { FileTree } from './FileTree.js';

const root = '/Library/Active notes';
const files: WorkspaceFile[] = [
  {
    path: `${root}/notes/target.md`,
    name: 'target.md',
    dir: 'notes',
    mtime: 1,
    size: 12,
    ignored: false,
  },
  {
    path: `${root}/other.md`,
    name: 'other.md',
    dir: '',
    mtime: 2,
    size: 8,
    ignored: false,
  },
];

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(260);
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(220);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it('reopens a collapsed directory when a filter targets one of its documents', async () => {
  const onOpen = vi.fn();
  const tree = (filter: string) => (
    <FileTree
      files={files}
      root={root}
      activePath={null}
      onOpen={onOpen}
      filter={filter}
    />
  );
  const view = render(tree(''));

  const notes = await screen.findByRole('treeitem', { name: 'notes' });
  expect(notes.getAttribute('aria-expanded')).toBe('true');
  expect(screen.getByRole('treeitem', { name: 'target.md' })).toBeTruthy();

  fireEvent.click(notes);
  await waitFor(() => {
    expect(screen.getByRole('treeitem', { name: 'notes' }).getAttribute('aria-expanded')).toBe(
      'false',
    );
  });
  expect(screen.queryByRole('treeitem', { name: 'target.md' })).toBeNull();

  view.rerender(tree('target'));

  await waitFor(() => {
    expect(screen.getByRole('treeitem', { name: 'notes' }).getAttribute('aria-expanded')).toBe(
      'true',
    );
  });
  const target = screen.getByRole('treeitem', { name: 'target.md' });
  fireEvent.click(target);
  expect(onOpen).toHaveBeenCalledWith(files[0]);
});
