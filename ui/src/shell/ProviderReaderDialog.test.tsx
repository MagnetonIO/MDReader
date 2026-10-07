import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { ProviderReaderDialog } from './ProviderReaderDialog';
import type { ProviderDocumentBody, ProviderReaderReference } from '../platform/providers';

afterEach(cleanup);
const reference: ProviderReaderReference = { sourceId: `provider:${'a'.repeat(64)}`, path: `records/${'b'.repeat(64)}.md`, revision: 'c'.repeat(64), title: 'Selected note' };
function body(ref = reference, markdown = '# Selected content\n\n![Private image](https://example.invalid/private.png)'): ProviderDocumentBody {
  return { ...ref, operation: 'read_document', markdown, lineStart: 1, lineEnd: 3 };
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
  const view = render(<ProviderReaderDialog reference={reference} backend={backend} onClose={() => {}} />);
  view.rerender(<ProviderReaderDialog reference={next} backend={backend} onClose={() => {}} />);
  await screen.findByRole('heading', { name: /Current selection/ });
  await act(async () => finish(body(reference, '# Stale selection')));
  expect(screen.queryByRole('heading', { name: /Stale selection/ })).toBeNull();
  view.rerender(<ProviderReaderDialog reference={reference} backend={{ readDocument: async () => body(next, '# Wrong revision') }} onClose={() => {}} />);
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', 'The selected document revision is no longer available.');
  expect(screen.queryByRole('heading', { name: /Wrong revision/ })).toBeNull();
  expect(screen.queryByText('Reading Markdown…')).toBeNull();
});
