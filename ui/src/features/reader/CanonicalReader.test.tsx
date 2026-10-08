import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { CanonicalReader } from './CanonicalReader';
import type { CanonicalReaderBackend } from './contracts';
import type { ProviderReaderReference } from '../../platform/providers';

afterEach(cleanup);
test('inline local links expose exact ambiguous candidates and preserve shared previous navigation', async () => {
  const origin: ProviderReaderReference = {
    sourceId: 'a'.repeat(64), path: 'origin.md', revision: 'b'.repeat(64), title: 'Origin',
  };
  const candidates = ['one/note.md', 'two/note.md'].map((path) => ({
    ...origin, path, revision: 'c'.repeat(64), title: path, line: 3,
  }));
  const backend: CanonicalReaderBackend = {
    readDocument: vi.fn(async (reference) => ({
      operation: 'read_document' as const, ...reference,
      markdown: reference.path === origin.path ? '# Origin\n\n[Next](Shared%20title)' : '# Selected exact copy',
      lineStart: 1, lineEnd: 3,
    })),
    resolveLink: vi.fn(async () => ({ state: 'local_matches' as const, references: candidates })),
  };
  render(<CanonicalReader reference={origin} backend={backend} sourceKind="local" onClose={() => {}} />);
  fireEvent.click(await screen.findByRole('link', { name: 'Next' }));
  expect(backend.resolveLink).toHaveBeenCalledWith(origin, 'relative', 'Shared%20title');
  expect(backend.readDocument).toHaveBeenCalledOnce();
  fireEvent.click(await screen.findByRole('button', { name: 'Read exact two/note.md' }));
  await screen.findByRole('heading', { name: /Selected exact copy/ });
  expect(backend.readDocument).toHaveBeenLastCalledWith(candidates[1]);
  expect(screen.queryByRole('dialog')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Previous document' }));
  await screen.findByRole('heading', { name: /Origin/ });
  expect(backend.readDocument).toHaveBeenLastCalledWith(origin);
});

test('restoring a saved position waits for exact receipt verification and refuses a mismatched body', async () => {
  const reference = { sourceId: 'a'.repeat(64), path: 'saved.md', revision: 'b'.repeat(64), title: 'Saved' };
  const onPosition = vi.fn();
  const backend: CanonicalReaderBackend = {
    readDocument: vi.fn(async () => ({ operation: 'read_document' as const, ...reference,
      revision: 'c'.repeat(64), markdown: '# Different revision', lineStart: 1, lineEnd: 1 })),
  };
  render(<CanonicalReader reference={reference} backend={backend} sourceKind="local" onClose={() => {}}
    position={{ revision: reference.revision, line: 1, block: 0, offset: 10 }} onPosition={onPosition} />);
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', 'The selected document revision is no longer available.');
  expect(screen.queryByRole('heading', { name: /Different revision/ })).toBeNull();
  expect(onPosition).not.toHaveBeenCalled();
});

test('an exact capture read disables local editing and archive comparison even after history restore', async () => {
  const reference = { sourceId: 'a'.repeat(64), path: 'outputs/plan.md', revision: 'b'.repeat(64), title: 'Captured plan' };
  const backend: CanonicalReaderBackend = { readDocument: vi.fn(async () => ({ operation: 'read_document' as const,
    ...reference, sourceKind: 'capture' as const, markdown: '# Visible plan', lineStart: 1, lineEnd: 1 })) };
  const edit = vi.fn(); const reviews = vi.fn();
  render(<CanonicalReader reference={reference} backend={backend} sourceKind="local" onClose={() => {}} onEdit={edit} onArchivedReviews={reviews} />);
  await screen.findByRole('heading', { name: /Visible plan/ });
  expect(screen.queryByRole('button', { name: 'Edit local file' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Archived reviews…' })).toBeNull();
  expect(screen.getByText(/exact captured visible-output revision/)).toBeTruthy();
  expect(edit).not.toHaveBeenCalled(); expect(reviews).not.toHaveBeenCalled();
});
