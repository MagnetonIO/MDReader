import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { readingLink, parseReadingLink } from './reading-link';
import { exportNativeMarkdown } from './native-export';
import { DocumentActions } from './DocumentActions';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const reference = { sourceId: 'a'.repeat(64), path: 'note.md', revision: 'b'.repeat(64), title: 'Note' };
afterEach(() => { cleanup(); vi.resetAllMocks(); vi.unstubAllGlobals(); });

test('exact reading link round-trips source/revision/line and rejects extra grants or escaping paths', () => {
  expect(parseReadingLink(readingLink(reference, 12))).toMatchObject({ ...reference, title: '', line: 12 });
  const link = new URL(readingLink(reference)); link.searchParams.set('grant', '/private');
  expect(() => parseReadingLink(link.href)).toThrow('No source will be authorized');
  expect(() => readingLink({ ...reference, path: '../escape.md' })).toThrow();
});
test('native export sends only an exact receipt and distinguishes cancellation from unconfirmed write', async () => {
  vi.mocked(invoke).mockResolvedValue({ state: 'cancelled' });
  expect(await exportNativeMarkdown(reference)).toBe('cancelled');
  expect(invoke).toHaveBeenCalledWith('reader_export_document', { reference: {
    documentId: null, sourceId: reference.sourceId, path: reference.path, revision: reference.revision,
  } });
  vi.mocked(invoke).mockRejectedValue({ kind: 'write_unconfirmed' });
  await expect(exportNativeMarkdown(reference)).rejects.toThrow('Check the destination');
});
test('copy local link describes a connected-library bookmark and retains the exact receipt', async () => {
  const writeText = vi.fn(async (_value: string) => {});
  vi.stubGlobal('navigator', { clipboard: { writeText } });
  render(<DocumentActions reference={reference} read={vi.fn()} line={12} />);
  const copy = screen.getByRole('button', { name: 'Copy local link' });
  expect(copy.getAttribute('title')).toBe('Bookmark this exact version in your connected library');
  fireEvent.click(copy);
  await screen.findByText('Local reading link copied.');
  expect(parseReadingLink(writeText.mock.calls[0]![0])).toMatchObject({
    sourceId: reference.sourceId, path: reference.path, revision: reference.revision, line: 12,
  });
});
test('late copy preparation cannot publish a body for a newly selected revision', async () => {
  let resolve!: (value: { operation: 'read_document'; sourceId: string; path: string; revision: string; markdown: string; lineStart: number; lineEnd: number }) => void;
  const read = vi.fn(() => new Promise<Parameters<typeof resolve>[0]>((done) => { resolve = done; }));
  const create = vi.fn();
  vi.stubGlobal('URL', class extends URL { static override createObjectURL = create; });
  const view = render(<DocumentActions reference={reference} read={read} />);
  fireEvent.click(screen.getByRole('button', { name: 'Export Markdown' }));
  const next = { ...reference, revision: 'c'.repeat(64) };
  view.rerender(<DocumentActions reference={next} read={read} />);
  await act(async () => { resolve({ operation: 'read_document', ...reference, markdown: '# Old body', lineStart: 1, lineEnd: 1 }); });
  expect(create).not.toHaveBeenCalled();
  expect(screen.queryByRole('textbox', { name: 'Reading link' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Reading link details' }));
  expect(parseReadingLink(screen.getByRole('textbox', { name: 'Reading link' }).getAttribute('value')!).revision).toBe(next.revision);
});
