import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import type { AssetManifest, AssetReceipt, AssetReply, AssetRequest } from '../../bindings';
import type { AssetBackend } from '../../mobile/device-assets';
import { DeviceReader } from './DeviceReader';
import type { DeviceReaderBackend, DeviceReference } from './backend';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
const reference: DeviceReference = {
  documentId: '00000000-0000-4000-8000-000000000001', sourceId: 'a'.repeat(64),
  path: 'document.md', revision: 'b'.repeat(64), title: 'Received note', line: 1,
};
const receipt: AssetReceipt = {
  asset_id: '00000000-0000-4000-8000-000000000002', document_id: reference.documentId,
  document_revision: reference.revision, target: 'pixel.png', content_hash: 'c'.repeat(64),
  mime: 'image/png', bytes: 3, width: 1, height: 1,
};
const manifest: AssetManifest = {
  document_id: reference.documentId, document_revision: reference.revision, manifest: 'd'.repeat(64),
  assets: [receipt], pending: [], targets: ['pixel.png'], targets_truncated: false,
};
test('normal device reader loads only selected visible images and retires a late lease on selection change', async () => {
  let visibility!: (entries: Partial<IntersectionObserverEntry>[]) => void;
  vi.stubGlobal('IntersectionObserver', class {
    constructor(callback: typeof visibility, options?: IntersectionObserverInit) {
      if (options?.rootMargin === '200px') visibility = callback;
    }
    observe() {} disconnect() {}
  });
  const create = vi.fn(() => 'blob:selected');
  const revoke = vi.fn();
  vi.stubGlobal('URL', class extends URL {
    static override createObjectURL = create;
    static override revokeObjectURL = revoke;
  });
  let finish!: (value: AssetReply) => void;
  const images: AssetBackend = vi.fn(async (request: AssetRequest): Promise<AssetReply> => {
    if (request.operation === 'open') return new Promise<AssetReply>((resolve) => { finish = resolve; });
    if (request.operation === 'close') return { operation: 'complete' };
    throw new Error('No chunk after selection change');
  });
  const api: DeviceReaderBackend = {
    read: vi.fn(async (value) => ({ operation: 'read_document', ...value,
      markdown: '# Device copy\n\n![Selected](pixel.png)\n\n![Unselected](https://example.invalid/private.png)', lineStart: 1, lineEnd: 5 })),
    manifest: vi.fn(async (value) => value === reference ? manifest : null),
    images: vi.fn(() => images),
  };
  const view = render(<DeviceReader reference={reference} backend={api} onClose={() => {}} />);
  await screen.findByRole('heading', { name: /Device copy/ });
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(screen.getByText(/Unselected.*asset is not available/)).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Load remote image' })).toBeNull();
  expect(images).not.toHaveBeenCalled();
  await waitFor(() => expect(visibility).toBeTypeOf('function'));
  act(() => visibility([{ isIntersecting: true }]));
  await waitFor(() => expect(images).toHaveBeenCalledWith(expect.objectContaining({ operation: 'open', document_id: reference.documentId, revision: reference.revision })));
  const next = { ...reference, documentId: '00000000-0000-4000-8000-000000000003', revision: 'e'.repeat(64) };
  view.rerender(<DeviceReader reference={next} backend={api} onClose={() => {}} />);
  await act(async () => finish({ operation: 'opened', session_id: 'retired', receipt }));
  await waitFor(() => expect(images).toHaveBeenCalledWith({ operation: 'close', session_id: 'retired' }));
  expect(create).not.toHaveBeenCalled();
  expect(revoke).not.toHaveBeenCalled();
  expect(screen.queryByRole('img')).toBeNull();
});

test('loaded Blob URLs are revoked when the exact device selection unmounts', async () => {
  let visibility!: (entries: Partial<IntersectionObserverEntry>[]) => void;
  vi.stubGlobal('IntersectionObserver', class { constructor(callback: typeof visibility, options?: IntersectionObserverInit) {
      if (options?.rootMargin === '200px') visibility = callback;
    } observe() {} disconnect() {} });
  const revoke = vi.fn();
  vi.stubGlobal('URL', class extends URL { static override createObjectURL = () => 'blob:device'; static override revokeObjectURL = revoke; });
  const images: AssetBackend = vi.fn(async (request: AssetRequest): Promise<AssetReply> => {
    if (request.operation === 'open') return { operation: 'opened', session_id: 'selected', receipt };
    if (request.operation === 'chunk') return { operation: 'chunk', offset: 0, base64: 'AQID', complete: true };
    if (request.operation === 'close') return { operation: 'complete' };
    throw new Error('Read-only transport');
  });
  const api: DeviceReaderBackend = {
    read: async (value) => ({ operation: 'read_document', ...value, markdown: '# Device\n\n![Selected](pixel.png)', lineStart: 1, lineEnd: 3 }),
    manifest: async () => manifest, images: () => images,
  };
  const view = render(<DeviceReader reference={reference} backend={api} onClose={() => {}} />);
  await screen.findByRole('heading', { name: /Device/ });
  await waitFor(() => expect(visibility).toBeTypeOf('function'));
  act(() => visibility([{ isIntersecting: true }]));
  expect((await screen.findByRole('img', { name: 'Selected' })).getAttribute('src')).toBe('blob:device');
  view.unmount();
  expect(revoke).toHaveBeenCalledWith('blob:device');
  expect(images).toHaveBeenCalledWith({ operation: 'close', session_id: 'selected' });
});
