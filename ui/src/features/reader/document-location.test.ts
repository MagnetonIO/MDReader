import { expect, test, vi } from 'vitest';
import { resolveDocumentLocation } from './document-location';
import { nativeProvidersBackend, type ProviderSource } from '../../platform/providers';
import type { LibrarySnapshot } from '../../bindings';
const reference = { sourceId: 'a'.repeat(64), revision: 'b'.repeat(64), path: 'notes/index.md' };
function local(root = '/Users/public/Selected notes') {
  const exact = { ...reference, librarySourceId: 'member', root, path: `${root}/notes/index.md`, relativePath: reference.path };
  const snapshot: LibrarySnapshot = { version: 4, sources: [{ id: 'member', name: 'Selected notes', path: root, state: { kind: 'available' }, index_state: 'present', active: false, restore_candidate: false, writable: true }], cloud_sources: [], cleanup: { kind: 'complete' } };
  return { local: vi.fn(async () => exact), libraries: vi.fn(async () => snapshot), providers: nativeProvidersBackend };
}
test('local full path comes from the exact native resolver and friendly name from its matching member', async () => {
  const backend = local();
  expect(await resolveDocumentLocation(reference, 'local', () => true, backend)).toEqual({ file: 'index.md', source: 'Selected notes', path: '/Users/public/Selected notes/notes/index.md' });
  expect(backend.local).toHaveBeenCalledExactlyOnceWith(reference);
});
test('preserves the native Windows path exactly rather than synthesizing separators', async () => {
  const backend = local('C:\\Public notes');
  backend.local.mockResolvedValue({ ...await backend.local(), path: 'C:\\Public notes\\notes\\index.md' });
  expect((await resolveDocumentLocation(reference, 'local', () => true, backend))?.path).toBe('C:\\Public notes\\notes\\index.md');
});
test('an unavailable exact local receipt cannot expose the stored descriptor path', async () => {
  const backend = local(); backend.local.mockRejectedValue(new Error('unauthorized'));
  await expect(resolveDocumentLocation(reference, 'local', () => true, backend)).rejects.toThrow('unauthorized');
  expect(backend.libraries).not.toHaveBeenCalled();
});
test('capture and device archives do not acquire a local original path', async () => {
  const backend = local();
  expect(await resolveDocumentLocation(reference, 'capture', () => true, backend)).toBeNull();
  expect(await resolveDocumentLocation({ ...reference, documentId: 'device' }, 'device', () => true, backend)).toBeNull();
  expect(backend.local).not.toHaveBeenCalled(); expect(backend.libraries).not.toHaveBeenCalled();
});
function cloud(kind: ProviderSource['kind']) {
  const selected = { ...reference, sourceId: `provider:${'c'.repeat(64)}`, path: `records/${'d'.repeat(64)}.md`, title: 'Public.md' };
  const source: ProviderSource = { sourceId: 'saved', readerSourceId: selected.sourceId, name: 'Public account', kind, enabled: true,
    selectedScope: kind === 'dropbox' ? { kind: 'scoped_root', recursive: true, path: '/notes' } : { kind: 'drive_folder', recursive: true, folderId: 'folder-opaque-id' },
    capabilities: { read: true, list: true, changes: true, assets: false, guardedWrite: false }, health: 'ready', coverage: { kind: 'never_indexed' } };
  const document = { documentId: 'native-document', name: 'Public.md', displayPath: kind === 'dropbox' ? '/notes/public.md' : '/folder-opaque-id/file-opaque-id', modifiedAtMs: null, sizeBytes: null, cached: true, cachedObservedAtMs: null, admitted: true, projection: { jobId: 'job', readerSourceId: selected.sourceId, path: selected.path, revision: selected.revision } };
  const providers = { ...nativeProvidersBackend, state: vi.fn(async () => ({ sources: [source], availability: [], pendingAdmissions: [], error: null })), documents: vi.fn(async () => ({ operation: 'documents' as const, documents: [document], offset: 0, nextOffset: null })) };
  return { selected, document, backend: { ...local(), providers } };
}
test('Drive exposes its saved label and filename, never opaque ID breadcrumb or cache disk path', async () => {
  const { selected, backend } = cloud('google_drive');
  expect(await resolveDocumentLocation(selected, 'provider', () => true, backend)).toEqual({ file: 'Public.md', source: 'Google Drive · Public account', location: 'Selected Drive folder' });
  expect(backend.local).not.toHaveBeenCalled();
});
test('Dropbox location requires the unique exact admitted projection, not a matching filename', async () => {
  const { selected, document, backend } = cloud('dropbox');
  expect(await resolveDocumentLocation(selected, 'provider', () => true, backend)).toEqual({ file: 'Public.md', source: 'Dropbox · Public account', location: '/notes/public.md', locationLabel: 'Dropbox path' });
  backend.providers.documents.mockResolvedValue({ operation: 'documents', documents: [{ ...document, displayPath: '/other/public.md', projection: { ...document.projection, path: `records/${'e'.repeat(64)}.md` } }], offset: 0, nextOffset: null });
  expect(await resolveDocumentLocation(selected, 'provider', () => true, backend)).toMatchObject({ location: '/notes', locationLabel: 'Dropbox folder' });
});
test('metadata unavailability keeps the saved provider label without exposing record keys', async () => {
  const { selected, backend } = cloud('google_drive');
  backend.providers.documents.mockRejectedValue(new Error('metadata unavailable'));
  expect(await resolveDocumentLocation({ ...selected, title: selected.path }, 'provider', () => true, backend)).toEqual({ file: 'Cloud document', source: 'Google Drive · Public account', location: 'Selected Drive folder' });
});
