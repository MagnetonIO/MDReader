import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { useLayoutEffect } from 'react';
import { afterEach, expect, test, vi } from 'vitest';
import { ProviderReaderDialog } from './ProviderReaderDialog';
import type { ProviderDocumentBody, ProviderReaderReference, ProviderLinkResolution, NativeProvidersBackend } from '../platform/providers';

afterEach(cleanup);
const reference: ProviderReaderReference = { sourceId: `provider:${'a'.repeat(64)}`, path: `records/${'b'.repeat(64)}.md`, revision: 'c'.repeat(64), title: 'Selected note' };
function body(ref = reference, markdown = '# Selected content\n\n![Private image](https://example.invalid/private.png)'): ProviderDocumentBody {
  return { ...ref, operation: 'read_document', markdown, lineStart: 1, lineEnd: 3 };
}
function ObservedDialog({ reference, backend, snapshot }: { reference: ProviderReaderReference; backend: Pick<NativeProvidersBackend, 'readDocument'>; snapshot: (value: string) => void }) {
  useLayoutEffect(() => { snapshot(document.querySelector('.provider-reader-dialog .mdreader')?.textContent ?? ''); }, [reference, snapshot]);
  return <ProviderReaderDialog reference={reference} backend={backend} onClose={() => {}} />;
}
test('reads the exact selected revision without fetching images and returns to sources', async () => {
  const close = vi.fn();
  const backend = { readDocument: vi.fn(async () => body()) };
  render(<ProviderReaderDialog reference={reference} backend={backend} onClose={close} />);
  expect(await screen.findByRole('heading', { name: /Selected content/ })).toBeTruthy();
  expect(backend.readDocument).toHaveBeenCalledWith(reference);
  expect(screen.queryByRole('img')).toBeNull();
  expect(screen.getByText(/asset is not available/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Back to sources' }));
  expect(close).toHaveBeenCalledOnce();
});
test('rejects a mismatched revision and ignores a late previous selection', async () => {
  let finish!: (value: ProviderDocumentBody) => void;
  const next = { ...reference, revision: 'd'.repeat(64) };
  const backend = { readDocument: vi.fn((ref: ProviderReaderReference) => ref === reference ? new Promise<ProviderDocumentBody>((resolve) => { finish = resolve; }) : Promise.resolve(body(next, '# Current selection'))) };
  const snapshots: string[] = [];
  const snapshot = (value: string) => { snapshots.push(value); };
  const view = render(<ObservedDialog reference={reference} backend={backend} snapshot={snapshot} />);
  view.rerender(<ObservedDialog reference={next} backend={backend} snapshot={snapshot} />);
  await screen.findByRole('heading', { name: /Current selection/ });
  await act(async () => finish(body(reference, '# Stale selection')));
  expect(screen.queryByRole('heading', { name: /Stale selection/ })).toBeNull();
  view.rerender(<ObservedDialog reference={reference} backend={{ readDocument: async () => body(next, '# Wrong revision') }} snapshot={snapshot} />);
  expect(snapshots.at(-1)).not.toContain('Current selection');
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', 'The selected document revision is no longer available.');
  expect(screen.queryByRole('heading', { name: /Wrong revision/ })).toBeNull();
  expect(screen.queryByText('Reading Markdown…')).toBeNull();
});
const candidate = { sourceId: '11111111-1111-4111-8111-111111111111', documentId: '22222222-2222-4222-8222-222222222222', title: 'Linked note' };
const projection = { jobId: 'synthetic-job', readerSourceId: reference.sourceId, path: `records/${'e'.repeat(64)}.md`, revision: 'f'.repeat(64) };
const linked = { sourceId: projection.readerSourceId, path: projection.path, revision: projection.revision, title: candidate.title, line: 1 };
test('follows a cached logical link using its exact receipt and returns to the previous document', async () => {
  const backend = {
    readDocument: vi.fn(async (ref: ProviderReaderReference) => ref.path === reference.path ? body(ref, '# Original\n\n[Next](Next.md)') : body(ref, '# Linked content')),
    resolveLink: vi.fn(async (): Promise<ProviderLinkResolution> => ({ state: 'resolved', candidate, projection, line: 1, heading: null })),
  };
  const archived = vi.fn();
  render(<ProviderReaderDialog reference={reference} backend={backend} onClose={() => {}} onArchivedReviews={archived} />);
  fireEvent.click(await screen.findByRole('link', { name: 'Next' }));
  await screen.findByRole('heading', { name: /Linked content/ });
  expect(backend.resolveLink).toHaveBeenCalledWith(reference, 'relative', 'Next.md');
  expect(backend.readDocument).toHaveBeenCalledWith(linked);
  fireEvent.click(screen.getByText('Document details', { selector: 'summary' }));
  fireEvent.click(screen.getByText('Import tools', { selector: 'summary' }));
  fireEvent.click(screen.getByRole('button', { name: 'Archived reviews…' }));
  expect(archived).toHaveBeenCalledWith(linked);
  fireEvent.click(screen.getByRole('button', { name: 'Previous document' }));
  await screen.findByRole('heading', { name: /Original/ });
});
test('requires an explicit fetch for a selected uncached link', async () => {
  const backend = {
    readDocument: vi.fn(async (ref: ProviderReaderReference) => ref.path === reference.path ? body(ref, '# Original\n\n[Next](Next.md)') : body(ref, '# Downloaded content')),
    resolveLink: vi.fn(async (): Promise<ProviderLinkResolution> => ({ state: 'selected_uncached', candidate, heading: 'downloaded-content' })),
    fetchCurrent: vi.fn(async () => projection),
  };
  render(<ProviderReaderDialog reference={reference} backend={backend} onClose={() => {}} />);
  fireEvent.click(await screen.findByRole('link', { name: 'Next' }));
  const fetch = await screen.findByRole('button', { name: 'Fetch current Linked note' });
  expect(backend.fetchCurrent).not.toHaveBeenCalled();
  expect(screen.getByRole('heading', { name: /Original/ })).toBeTruthy();
  fireEvent.click(fetch);
  await screen.findByRole('heading', { name: /Downloaded content/ });
  expect(screen.queryByText(/requested heading is unavailable/)).toBeNull();
  expect(backend.fetchCurrent).toHaveBeenCalledOnce();
  expect(backend.fetchCurrent).toHaveBeenCalledWith(candidate.sourceId, candidate.documentId);
});
test('ignores a late link resolution after the parent selects another document', async () => {
  let finish!: (value: ProviderLinkResolution) => void;
  const next = { ...reference, revision: 'd'.repeat(64) };
  const backend = {
    readDocument: vi.fn(async (ref: ProviderReaderReference) => body(ref, ref === reference ? '# Original\n\n[Next](Next.md)' : '# New parent selection')),
    resolveLink: vi.fn(() => new Promise<ProviderLinkResolution>((resolve) => { finish = resolve; })),
  };
  const view = render(<ProviderReaderDialog reference={reference} backend={backend} onClose={() => {}} />);
  fireEvent.click(await screen.findByRole('link', { name: 'Next' }));
  view.rerender(<ProviderReaderDialog reference={next} backend={backend} onClose={() => {}} />);
  await screen.findByRole('heading', { name: /New parent selection/ });
  await act(async () => finish({ state: 'resolved', candidate, projection, line: 1, heading: null }));
  expect(backend.readDocument).toHaveBeenCalledTimes(2);
  expect(screen.getByRole('heading', { name: /New parent selection/ })).toBeTruthy();
});
test('a newer in-document jump supersedes a pending linked-document action', async () => {
  let finish!: (value: ProviderLinkResolution) => void;
  const backend = {
    readDocument: vi.fn(async (ref: ProviderReaderReference) => body(ref, '# Original\n\n[Next](Next.md) [Stay here](#section)\n\n## Section')),
    resolveLink: vi.fn(() => new Promise<ProviderLinkResolution>((resolve) => { finish = resolve; })),
  };
  render(<ProviderReaderDialog reference={reference} backend={backend} onClose={() => {}} />);
  fireEvent.click(await screen.findByRole('link', { name: 'Next' }));
  fireEvent.click(screen.getByRole('link', { name: 'Stay here' }));
  await act(async () => finish({ state: 'resolved', candidate, projection, line: 1, heading: null }));
  expect(backend.readDocument).toHaveBeenCalledOnce();
  expect(screen.queryByText('Opening linked document…')).toBeNull();
  expect(screen.getByRole('heading', { name: /Original/ })).toBeTruthy();
});
test('local catalog revision uses the shared reader without provider freshness claims or image fetches', async () => {
  const local = { sourceId: '1'.repeat(64), path: 'plan.md', revision: 'b'.repeat(64), title: 'Local plan' };
  const backend = { readDocument: vi.fn(async () => body(local, '# Local exact body\n\n![Selected](https://example.invalid/image.png)')) };
  render(<ProviderReaderDialog reference={local} sourceKind="local" backend={backend} onClose={() => {}} />);
  expect(await screen.findByRole('heading', { name: /Local exact body/ })).toBeTruthy();
  expect(screen.getByRole('dialog', { name: 'Catalog document' })).toBeTruthy();
  expect(screen.queryByText(/newer provider revision/)).toBeNull();
  expect(screen.queryByRole('img')).toBeNull();
  expect(backend.readDocument).toHaveBeenCalledWith(local);
});
