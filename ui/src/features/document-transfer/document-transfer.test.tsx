import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { readingLink, parseReadingLink } from './reading-link';
import { exportNativeMarkdown } from './native-export';
import { DocumentActions } from './DocumentActions';
import { documentFilename } from './filename';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const reference = { sourceId: 'a'.repeat(64), path: 'note.md', revision: 'b'.repeat(64), title: 'Note' };
afterEach(() => { cleanup(); vi.resetAllMocks(); vi.unstubAllGlobals(); });

test('exact reading link round-trips source/revision/line and rejects extra grants or escaping paths', () => {
  expect(parseReadingLink(readingLink(reference, 12))).toMatchObject({ ...reference, title: '', line: 12 });
  const link = new URL(readingLink(reference)); link.searchParams.set('grant', '/private');
  expect(() => parseReadingLink(link.href)).toThrow('No source will be authorized');
  expect(() => readingLink({ ...reference, path: '../escape.md' })).toThrow();
});
test('native export sends an exact receipt and filename hint, without destination authority', async () => {
  vi.mocked(invoke).mockResolvedValue({ state: 'cancelled' });
  expect(await exportNativeMarkdown(reference)).toBe('cancelled');
  expect(invoke).toHaveBeenCalledWith('reader_export_document', { reference: {
    documentId: null, sourceId: reference.sourceId, path: reference.path, revision: reference.revision, filename: 'note.md',
  } });
  vi.mocked(invoke).mockRejectedValue({ kind: 'write_unconfirmed' });
  await expect(exportNativeMarkdown(reference)).rejects.toThrow('Check the destination');
});
test('export names preserve source filenames and imported document names', () => {
  expect(documentFilename({ ...reference, path: 'folder/Release notes.markdown', title: 'Display heading' })).toBe('Release notes.markdown');
  expect(documentFilename({ ...reference, path: 'document.md', documentId: 'device', title: 'jira-import_tickets_RM-059.md' })).toBe('jira-import_tickets_RM-059.md');
  expect(documentFilename({ ...reference, documentId: 'device', title: 'Notes: draft?' })).toBe('Notes_ draft_.md');
  expect(documentFilename({ ...reference, sourceId: `provider:${reference.sourceId}`, path: 'records/opaque.md', title: 'Cloud note.md' })).toBe('Cloud note.md');
});
test('normal document actions omit local bookmarks without publishing a web link', () => {
  render(<DocumentActions reference={reference} read={vi.fn()} />);
  expect(screen.getByRole('button', { name: 'Export Markdown' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: /copy.*link/i })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Reading link details' })).toBeNull();
  expect(screen.queryByRole('textbox', { name: 'Reading link' })).toBeNull();
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
  expect(screen.queryByRole('button', { name: 'Reading link details' })).toBeNull();
});
