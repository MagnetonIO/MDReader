import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { CanonicalReader } from './CanonicalReader';
const metadata = vi.hoisted(() => vi.fn());
vi.mock('./DocumentDetails', () => ({ DocumentDetails: (props: unknown) => { metadata(props); return <p>Public location fixture</p>; } }));
afterEach(() => { cleanup(); metadata.mockClear(); });
test('closed Document details does not mount location lookups; explicit expansion mounts and closing retires them', async () => {
  const reference = { sourceId: 'a'.repeat(64), revision: 'b'.repeat(64), path: 'index.md' };
  const backend = { readDocument: vi.fn(async () => ({ ...reference, operation: 'read_document' as const, sourceKind: 'local' as const, markdown: '# Public local document', lineStart: 1, lineEnd: 1 })) };
  render(<CanonicalReader reference={reference} backend={backend} sourceKind="local" documentActions={false} onClose={() => {}} />);
  await screen.findByRole('heading', { name: /Public local document/ });
  expect(metadata).not.toHaveBeenCalled();
  fireEvent.click(screen.getByText('Document details'));
  await screen.findByText('Public location fixture');
  expect(metadata).toHaveBeenLastCalledWith(expect.objectContaining({ reference, kind: 'local', verified: true }));
  fireEvent.click(screen.getByText('Document details'));
  await waitFor(() => expect(screen.queryByText('Public location fixture')).toBeNull());
});
