import { afterEach, expect, test, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { ReaderRecents } from './ReaderRecents';
import { useReaderHistoryStore } from './store';
import type { ReaderReference } from './model';

const reference = (index: number): ReaderReference => ({
  kind: 'local', sourceId: 'a'.repeat(64), path: `notes/long-original-filename-${index}.md`,
  revision: String(index).repeat(64),
});
afterEach(() => { cleanup(); useReaderHistoryStore.getState().reset(); });

test('one compact list opens each exact receipt once and exposes its full filename', () => {
  for (let index = 1; index <= 8; index++) useReaderHistoryStore.getState().open(reference(index), `Long document title ${index}`);
  const onOpen = vi.fn(); render(<ReaderRecents onOpen={onOpen} />);
  const list = screen.getByRole('list', { name: 'Recent documents' });
  expect(within(list).getAllByRole('listitem')).toHaveLength(8);
  expect(screen.queryByText(/Manage recent reads|All recent reads/)).toBeNull();
  const open = within(list).getByRole('button', { name: 'Long document title 8' });
  expect(open.title).toBe('Long document title 8 — notes/long-original-filename-8.md');
  fireEvent.click(open);
  expect(onOpen).toHaveBeenCalledTimes(1);
  expect(onOpen).toHaveBeenCalledWith(expect.objectContaining({
    sourceId: reference(8).sourceId, path: 'notes/long-original-filename-8.md', revision: '8'.repeat(64),
  }));
});

test('icon removal preserves open tabs and moves keyboard focus to the next row or empty heading', () => {
  useReaderHistoryStore.getState().open(reference(1), 'First');
  useReaderHistoryStore.getState().open(reference(2), 'Second');
  const tabs = useReaderHistoryStore.getState().data.tabs;
  render(<ReaderRecents onOpen={vi.fn()} />);
  const remove = screen.getByRole('button', { name: /Remove Second .* from recent documents/ });
  expect(remove.textContent).toBe('');
  remove.focus(); fireEvent.click(remove);
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'First' }));
  expect(useReaderHistoryStore.getState().data.tabs).toBe(tabs);
  fireEvent.click(screen.getByRole('button', { name: /Remove First .* from recent documents/ }));
  expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Recent' }));
  expect(screen.getByText('No recent documents.')).toBeTruthy();
});


test('one Recent list combines scoped workspace and receipt history without changing either opening authority', () => {
  const workspace = { path: '/public/notes/index.md', name: 'index.md' };
  const exact = reference(1);
  const openWorkspace = vi.fn(); const openExact = vi.fn();
  const view = render(<ReaderRecents onOpen={openExact} workspaceRecent={[workspace]} onOpenWorkspace={openWorkspace} />);
  expect(screen.getAllByRole('heading', { name: 'Recent' })).toHaveLength(1);
  expect(screen.queryByText('No recent documents.')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'index.md' }));
  expect(openWorkspace).toHaveBeenCalledExactlyOnceWith(workspace);
  expect(openExact).not.toHaveBeenCalled();
  useReaderHistoryStore.getState().open(exact, 'Exact note');
  view.rerender(<ReaderRecents onOpen={openExact} workspaceRecent={[workspace]} onOpenWorkspace={openWorkspace} />);
  expect(screen.getAllByRole('list', { name: 'Recent documents' })).toHaveLength(1);
  expect(screen.getAllByRole('listitem')).toHaveLength(2);
  fireEvent.click(screen.getByRole('button', { name: 'Exact note' }));
  expect(openExact).toHaveBeenCalledWith(expect.objectContaining({ path: exact.kind === 'local' ? exact.path : '', sourceId: exact.sourceId, revision: exact.revision }));
  fireEvent.click(screen.getByRole('button', { name: /Remove Exact note/ }));
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'index.md' }));
  expect(screen.queryByText('No recent documents.')).toBeNull();
});


test('saved revisions share one Recent row with the latest exact receipt; removal preserves revision-bound history', () => {
  const old: ReaderReference = { kind: 'provider', sourceId: `provider:${'a'.repeat(64)}`, path: `records/${'c'.repeat(64)}.md`, revision: '1'.repeat(64) };
  const current: ReaderReference = { ...old, revision: '2'.repeat(64) };
  const otherFolder: ReaderReference = { ...old, path: `records/${'d'.repeat(64)}.md` };
  const otherSource: ReaderReference = { ...old, sourceId: `provider:${'b'.repeat(64)}` };
  const store = useReaderHistoryStore.getState();
  store.open(old, 'Public note.md');
  const id = useReaderHistoryStore.getState().data.activeTabId!;
  const position = { revision: old.revision, line: 31, block: 4, offset: 7 };
  store.position(id, old, useReaderHistoryStore.getState().selection, position);
  store.open(otherFolder, 'Other folder'); store.open(otherSource, 'Other source');
  // Save normally navigates the active exact selection; preserve the old receipt entry.
  store.select(id);
  store.navigate(id, old, useReaderHistoryStore.getState().selection, { reference: current, title: 'Public note.md', heading: null, position: null });
  const retained = useReaderHistoryStore.getState().data;
  const open = vi.fn(); render(<ReaderRecents onOpen={open} />);
  expect(screen.getAllByRole('listitem')).toHaveLength(3);
  fireEvent.click(screen.getByRole('button', { name: 'Public note.md' }));
  expect(open).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ sourceId: current.sourceId, path: current.path, revision: current.revision }));
  expect(retained.recent).toHaveLength(4);
  expect(retained.tabs.find(tab => tab.id === id)!.entries[0]!.position).toEqual(position);
  fireEvent.click(screen.getByRole('button', { name: `Remove Public note.md — ${current.path} from recent documents` }));
  expect(screen.getAllByRole('listitem')).toHaveLength(2);
  expect(useReaderHistoryStore.getState().data.tabs).toBe(retained.tabs);
  expect(useReaderHistoryStore.getState().data.recent.map(entry => entry.reference)).toEqual([otherSource, otherFolder]);
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Other source' }));
});
