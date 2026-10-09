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
