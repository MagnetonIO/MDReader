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
