import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import type { NativeProvidersBackend, NativeProvidersSnapshot, ProviderSource, ProviderDocument } from '../platform/providers.js';
import { nativeProvidersBackend, NativeProviderError, providerConnectionDiagnostic } from '../platform/providers.js';
const raw = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => raw);
import { NativeProvidersPanel } from './NativeProvidersPanel.js';
afterEach(cleanup);
const source = (sourceId = 'remote-source', name = 'Selected plans'): ProviderSource => ({ sourceId, readerSourceId: null, name, kind: 'google_drive', enabled: true, selectedScope: { kind: 'exact_documents', count: 1 }, capabilities: { read: true, list: true, changes: true, assets: false, guardedWrite: true }, health: 'ready', coverage: { kind: 'never_indexed' } });
const state = (sources: ProviderSource[] = []): NativeProvidersSnapshot => ({ availability: [{ provider: 'google_drive', configured: true, supported: true, diagnostic: null }, { provider: 'dropbox', configured: false, supported: true, diagnostic: 'Dropbox registration is not configured.' }], sources, pendingAdmissions: [], error: null });
const projection = { jobId: 'job', readerSourceId: `provider:${'a'.repeat(64)}`, path: 'records/selected-hash.md', revision: 'exact-revision' };
const doc: ProviderDocument = { documentId: 'document-handle', name: 'Plan.md', displayPath: 'Plans / Plan.md', modifiedAtMs: null, sizeBytes: 10, cached: true, cachedObservedAtMs: 123, admitted: true, projection };
function backend(snapshot = state()): NativeProvidersBackend { return { state: vi.fn(async () => snapshot), connect: vi.fn(), reconnect: vi.fn(), reconcile: vi.fn(), documents: vi.fn(async () => ({ operation: 'documents' as const, documents: [doc], offset: 0, nextOffset: null })), refresh: vi.fn(async () => ({ operation: 'refresh' as const, pages: 1, metadataComplete: true })), fetchCurrent: vi.fn(async () => projection), readCached: vi.fn(async () => projection), readDocument: vi.fn(async () => ({ operation: 'read_document' as const, markdown: 'body', sourceId: projection.readerSourceId, path: projection.path, revision: projection.revision, lineStart: 1, lineEnd: 1 })), resolveLink: vi.fn(async () => ({ state: 'missing' as const })), disable: vi.fn(async () => {}), forget: vi.fn(async () => {}), disconnect: vi.fn(async () => ({ credential: 'deleted' as const, revocation: 'revoked' as const })) }; }
const dropboxReady = (snapshot: NativeProvidersSnapshot) => { snapshot.availability[1] = { provider: 'dropbox', configured: true, supported: true, diagnostic: null }; return snapshot; };
async function addSource(provider: 'Google Drive' | 'Dropbox') {
  fireEvent.click(await screen.findByRole('button', { name: 'Add source' }));
  fireEvent.click(await screen.findByRole('menuitem', { name: new RegExp(`^${provider}`) }));
  return within(await screen.findByRole('region', { name: `Add ${provider}` }));
}
async function sourceAction(name: string, action: string | RegExp) {
  const item = await screen.findByRole('listitem', { name });
  fireEvent.click(within(item).getByRole('button', { name: `More actions for ${name}` }));
  fireEvent.click(within(item).getByRole('menuitem', { name: action }));
}

test('Dropbox reconnect preserves its saved folder, recursion and edit grant despite fresh connection form changes', async () => {
  const saved: ProviderSource = { ...source(), kind: 'dropbox', selectedScope: { kind: 'scoped_root', path: '/Public', recursive: false },
    capabilities: { ...source().capabilities, guardedWrite: false } };
  const api = backend(dropboxReady(state([saved])));
  api.reconnect = vi.fn(async () => ({ state: 'admitted' as const, sourceId: saved.sourceId, enabled: true, replayed: false }));
  render(<NativeProvidersPanel backend={api} />);
  const form = await addSource('Dropbox');
  fireEvent.change(form.getByRole('textbox', { name: 'Dropbox folder path' }), { target: { value: '/Different' } });
  fireEvent.click(form.getByRole('checkbox', { name: 'Include subfolders' }));
  fireEvent.click(form.getByRole('checkbox', { name: 'Allow editing' }));
  await sourceAction(saved.name, 'Reconnect');
  await waitFor(() => expect(api.reconnect).toHaveBeenCalledExactlyOnceWith(saved.sourceId, { path: '/Public', recursive: false }, false));
  expect(api.connect).not.toHaveBeenCalled();
});
test('Settings owns explicit inclusion of native-confirmed saved sources and enforces the 64-source search bound', async () => {
  const saved = { ...source(), readerSourceId: projection.readerSourceId };
  const api = backend(state([saved])); const select = vi.fn();
  const view = render(<NativeProvidersPanel backend={api} selectedProviderSources={[]} onProviderSelectionChange={select} />);
  fireEvent.click(await screen.findByRole('checkbox', { name: 'Include Selected plans in search' }));
  expect(select).toHaveBeenCalledWith([projection.readerSourceId]);
  expect(api.fetchCurrent).not.toHaveBeenCalled(); expect(api.connect).not.toHaveBeenCalled();
  view.rerender(<NativeProvidersPanel backend={api} selectedProviderSources={Array.from({ length: 64 }, (_, index) => `provider:${index.toString(16).padStart(64, '0')}`)} onProviderSelectionChange={select} />);
  select.mockClear();
  fireEvent.click(screen.getByRole('checkbox', { name: 'Include Selected plans in search' }));
  expect(await screen.findByText('Choose at most 64 cloud sources for search.')).toBeTruthy();
  expect(select).not.toHaveBeenCalled();
});
test('a hidden panel reads no status; showing it reads status without connecting and explains unavailable providers', async () => {
  const api = backend(); const view = render(<NativeProvidersPanel backend={api} active={false} />);
  expect(api.state).not.toHaveBeenCalled();
  view.rerender(<NativeProvidersPanel backend={api} active />);
  await waitFor(() => expect(api.state).toHaveBeenCalledTimes(1));
  view.rerender(<NativeProvidersPanel backend={api} active={false} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Add source' }));
  const google = await screen.findByRole('menuitem', { name: /^Google Drive/ });
  expect(google.getAttribute('aria-disabled')).toBeNull();
  const dropbox = screen.getByRole('menuitem', { name: /^Dropbox/ });
  expect(dropbox.getAttribute('aria-disabled')).toBe('true');
  expect(within(dropbox).getByText('Dropbox registration is not configured.')).toBeTruthy();
  fireEvent.click(dropbox);
  expect(screen.queryByRole('region', { name: 'Add Dropbox' })).toBeNull();
  expect(api.connect).not.toHaveBeenCalled(); expect(api.state).toHaveBeenCalledTimes(1);
});
test('Add source reveals one provider form with Connect and Cancel', async () => {
  render(<NativeProvidersPanel backend={backend()} />);
  expect(await screen.findByText(/No cloud sources yet/)).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Connect Google Drive' })).toBeNull();
  const form = await addSource('Google Drive');
  expect(screen.queryByText(/No cloud sources yet/)).toBeNull();
  expect(form.getByRole('button', { name: 'Connect Google Drive' })).toBeTruthy();
  expect(document.activeElement).toBe(form.getByRole('heading', { name: 'Add Google Drive' }));
  fireEvent.click(form.getByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('region', { name: 'Add Google Drive' })).toBeNull();
});
test('Drive read-only default, optional Picker and Dropbox actions pass only the explicit displayed selection', async () => {
  const api = backend(dropboxReady(state())); api.connect = vi.fn(async () => ({ state: 'admitted' as const, sourceId: 'new-source', enabled: true, replayed: false }));
  render(<NativeProvidersPanel backend={api} />);
  fireEvent.click((await addSource('Google Drive')).getByRole('button', { name: 'Connect Google Drive' }));
  await waitFor(() => expect(api.connect).toHaveBeenCalledWith('google_drive', null, false, { id: 'root', recursive: true }));
  await screen.findByText(/selected source is connected/);
  expect(screen.queryByRole('region', { name: 'Add Google Drive' })).toBeNull();
  fireEvent.click((await addSource('Google Drive')).getByRole('button', { name: 'Choose specific files instead' }));
  await waitFor(() => expect(api.connect).toHaveBeenCalledWith('google_drive', null, false));
  await screen.findByText(/selected source is connected/);
  let form = await addSource('Dropbox');
  fireEvent.change(form.getByRole('textbox', { name: 'Dropbox folder path' }), { target: { value: '/Notes' } });
  fireEvent.click(form.getByRole('checkbox', { name: 'Include subfolders' }));
  fireEvent.click(form.getByRole('button', { name: 'Connect Dropbox folder' }));
  await waitFor(() => expect(api.connect).toHaveBeenCalledWith('dropbox', { path: '/Notes', recursive: true }, false));
  await screen.findByText(/selected source is connected/);
  form = await addSource('Dropbox');
  fireEvent.change(form.getByRole('textbox', { name: 'Dropbox folder path' }), { target: { value: '/../outside' } });
  expect((form.getByRole('button', { name: 'Connect Dropbox folder' }) as HTMLButtonElement).disabled).toBe(true);
  expect(api.connect).toHaveBeenCalledTimes(3);
  fireEvent.change(form.getByRole('textbox', { name: 'Dropbox folder path' }), { target: { value: '/Notes' } });
  fireEvent.click(form.getByRole('checkbox', { name: 'Allow editing' }));
  fireEvent.click(form.getByRole('button', { name: 'Connect Dropbox folder' }));
  await waitFor(() => expect(api.connect).toHaveBeenLastCalledWith('dropbox', { path: '/Notes', recursive: true }, true));
  expect(api.connect).toHaveBeenCalledTimes(4);
});
test('Google options request fresh edit consent and exact recursion, while reconnect preserves its admitted folder options', async () => {
  const retained: ProviderSource = { ...source('drive-folder', 'Project notes'), selectedScope: { kind: 'drive_folder', folderId: 'project-id', recursive: false }, capabilities: { ...source().capabilities, browseFolders: true } };
  const api = backend(state([retained]));
  api.connect = vi.fn(async () => ({ state: 'admitted' as const, sourceId: 'new-source', enabled: true, replayed: false }));
  api.reconnect = vi.fn(async () => ({ state: 'admitted' as const, sourceId: retained.sourceId, enabled: true, replayed: false }));
  render(<NativeProvidersPanel backend={api} />);
  let google = await addSource('Google Drive');
  fireEvent.click(google.getByRole('checkbox', { name: 'Include subfolders' }));
  fireEvent.click(google.getByRole('checkbox', { name: 'Allow editing' }));
  fireEvent.click(google.getByRole('button', { name: 'Connect Google Drive' }));
  await waitFor(() => expect(api.connect).toHaveBeenCalledWith('google_drive', null, true, { id: 'root', recursive: false }));
  await screen.findByText(/selected source is connected/);
  google = await addSource('Google Drive');
  fireEvent.click(google.getByRole('checkbox', { name: 'Include subfolders' }));
  fireEvent.click(google.getByRole('checkbox', { name: 'Allow editing' }));
  await sourceAction(retained.name, 'Reconnect');
  await waitFor(() => expect(api.reconnect).toHaveBeenCalledWith(retained.sourceId, null, true, { id: 'project-id', recursive: false }));
  expect(api.fetchCurrent).not.toHaveBeenCalled();
});
test('metadata enumeration never claims indexed bodies and saved-copy reads use the exact core reference', async () => {
  const selected = source(); selected.coverage = { kind: 'metadata_complete_selected', observedAtMs: 10 };
  const api = backend(state([selected])); const onOpen = vi.fn();
  render(<NativeProvidersPanel backend={api} onOpen={onOpen} />);
  expect(await screen.findByText(/file list not loaded/)).toBeTruthy();
  expect(screen.queryByText(/documents indexed|Selected documents indexed/)).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Browse' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Open saved copy' }));
  await waitFor(() => expect(onOpen).toHaveBeenCalledWith({ sourceId: projection.readerSourceId, path: projection.path, revision: projection.revision, title: doc.name, line: 1 }));
  expect(api.readCached).toHaveBeenCalledWith(selected.sourceId, doc.documentId); expect(api.fetchCurrent).not.toHaveBeenCalled();
  await screen.findByText(/Opened the exact saved copy/);
  fireEvent.click(screen.getByRole('button', { name: 'Download latest' }));
  await waitFor(() => expect(api.fetchCurrent).toHaveBeenCalledWith(selected.sourceId, doc.documentId));
  await screen.findByText(/Indexing may still be in progress/); expect(onOpen).toHaveBeenCalledTimes(2);
  fireEvent.click(screen.getByRole('button', { name: 'Close documents' }));
  expect(screen.queryByRole('region', { name: `Documents in ${selected.name}` })).toBeNull();
});
test('native broad-Drive capability gates folder browsing and choosing a returned folder creates a fresh read-only connection', async () => {
  const broad: ProviderSource = { ...source('drive-source', 'Drive Markdown'), selectedScope: { kind: 'drive_readonly_folder', accountWide: true, folderId: 'root' },
    capabilities: { ...source().capabilities, guardedWrite: false, browseFolders: true } };
  const api = backend(state([broad]));
  api.browseFolders = vi.fn(async (_sourceId: string, folderId: string) => ({ operation: 'browse_folders' as const, folders: folderId === 'root' ? [{ id: 'project-id', name: 'Projects' }] : [], nextCursor: null }));
  api.connect = vi.fn(async () => ({ state: 'admitted' as const, sourceId: 'new-folder-source', enabled: true, replayed: false }));
  api.reconnect = vi.fn(async () => ({ state: 'admitted' as const, sourceId: broad.sourceId, enabled: true, replayed: false }));
  render(<NativeProvidersPanel backend={api} />);
  await screen.findByRole('listitem', { name: broad.name });
  expect(api.browseFolders).not.toHaveBeenCalled();
  await sourceAction(broad.name, 'Choose a Drive folder');
  fireEvent.click(await screen.findByRole('button', { name: 'Browse Projects' }));
  await screen.findByText('No child folders in this location.');
  fireEvent.click(screen.getByRole('button', { name: 'Connect this folder' }));
  await waitFor(() => expect(api.connect).toHaveBeenCalledWith('google_drive', null, false, { id: 'project-id', recursive: true }));
  expect(api.reconnect).not.toHaveBeenCalled(); expect(api.fetchCurrent).not.toHaveBeenCalled();
  await screen.findByText(/selected source is connected/);
  await sourceAction(broad.name, 'Reconnect');
  await waitFor(() => expect(api.reconnect).toHaveBeenCalledWith(broad.sourceId, null, false, { id: 'root' }));
});
test('a retained direct-folder Drive reconnect uses its native scope ID and never widens it to root or Picker', async () => {
  const direct: ProviderSource = { ...source('folder-source'), selectedScope: { kind: 'drive_readonly_folder', accountWide: false, folderId: 'retained-folder-id' },
    capabilities: { ...source().capabilities, browseFolders: true, guardedWrite: false } };
  const api = backend(state([direct]));
  api.reconnect = vi.fn(async () => ({ state: 'admitted' as const, sourceId: direct.sourceId, enabled: true, replayed: false }));
  render(<NativeProvidersPanel backend={api} />);
  await sourceAction(direct.name, 'Reconnect');
  await waitFor(() => expect(api.reconnect).toHaveBeenCalledWith(direct.sourceId, null, false, { id: 'retained-folder-id' }));
  fireEvent.click(screen.getByRole('button', { name: `More actions for ${direct.name}` }));
  expect(screen.queryByRole('menuitem', { name: 'Choose a Drive folder' })).toBeNull();
  expect(api.connect).not.toHaveBeenCalled();
});
test('a pending admission is reconciled explicitly without another OAuth attempt', async () => {
  const snapshot = state(); const api = backend(snapshot);
  api.connect = vi.fn(async () => { snapshot.pendingAdmissions = [{ attemptId: 'pending', provider: 'google_drive', name: 'Selected plans', sourceId: null, phase: 'admission_pending' }]; return { state: 'pending' as const, attemptId: 'pending', reason: { kind: 'timeout' } }; });
  api.reconcile = vi.fn(async () => { snapshot.pendingAdmissions = []; return { state: 'admitted' as const, sourceId: 'same-source', enabled: false, replayed: true }; });
  render(<NativeProvidersPanel backend={api} />);
  fireEvent.click((await addSource('Google Drive')).getByRole('button', { name: 'Connect Google Drive' }));
  expect(await screen.findByText(/connection outcome is pending/)).toBeTruthy(); expect(api.reconcile).not.toHaveBeenCalled(); expect(api.connect).toHaveBeenCalledTimes(1);
  fireEvent.click(await screen.findByRole('button', { name: 'Reconcile connection' }));
  await screen.findByText(/source remains paused/); expect(api.reconcile).toHaveBeenCalledWith('pending'); expect(api.connect).toHaveBeenCalledTimes(1);
});
test('profile changes discard late fetch results and never open a document in the new library', async () => {
  let resolve!: (value: typeof projection) => void; const old = backend(state([source()])); old.fetchCurrent = vi.fn(() => new Promise<typeof projection>((done) => { resolve = done; })); const fresh = backend(state([source('new-source', 'New library')])); const onOpen = vi.fn();
  const view = render(<NativeProvidersPanel backend={old} profileKey="old" onOpen={onOpen} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Browse' })); fireEvent.click(await screen.findByRole('button', { name: 'Download latest' }));
  await waitFor(() => expect(old.fetchCurrent).toHaveBeenCalledTimes(1)); view.rerender(<NativeProvidersPanel backend={fresh} profileKey="new" onOpen={onOpen} />);
  await screen.findByRole('heading', { name: 'New library' }); resolve(projection);
  await waitFor(() => expect(screen.queryByText(/Downloaded and opened/)).toBeNull()); expect(onOpen).not.toHaveBeenCalled(); expect(fresh.documents).not.toHaveBeenCalled();
});
test('More menu pause and refresh use the actual service source ID and update only after completion', async () => {
  const selected = source('actual-service-id'); const snapshot = state([selected]); const api = backend(snapshot); const changed = vi.fn();
  api.disable = vi.fn(async () => { selected.enabled = false; selected.health = 'paused'; });
  render(<NativeProvidersPanel backend={api} onSourcesChanged={changed} />);
  await sourceAction(selected.name, 'Refresh file list');
  await screen.findByText(/File list refreshed/); expect(api.refresh).toHaveBeenCalledWith('actual-service-id');
  await sourceAction(selected.name, 'Pause');
  await screen.findByText(/downloaded copies are no longer accessible/); expect(api.disable).toHaveBeenCalledWith('actual-service-id'); expect(changed).toHaveBeenCalledTimes(2);
  const item = screen.getByRole('listitem', { name: selected.name });
  expect(within(item).queryByRole('button', { name: 'Browse' })).toBeNull();
  expect(within(item).getByText('Paused')).toBeTruthy();
  expect(within(item).getByRole('button', { name: 'Reconnect' })).toBeTruthy();
  expect(api.forget).not.toHaveBeenCalled(); expect(api.disconnect).not.toHaveBeenCalled();
});
test('Disconnect asks inline, then removes the source with the actual service ID', async () => {
  const selected = source('actual-service-id'); const snapshot = state([selected]); const api = backend(snapshot); const changed = vi.fn();
  api.disconnect = vi.fn(async () => { snapshot.sources = []; return { credential: 'deleted' as const, revocation: 'revoked' as const }; });
  render(<NativeProvidersPanel backend={api} onSourcesChanged={changed} />);
  const item = await screen.findByRole('listitem', { name: selected.name });
  await sourceAction(selected.name, 'Disconnect…');
  const confirm = within(item).getByRole('group', { name: /Disconnect Selected plans\?/ });
  expect(confirm.textContent).toContain('Removes this account from MDReader, deletes downloaded copies and the saved sign-in. Files in Google Drive are not touched.');
  expect(document.activeElement).toBe(within(confirm).getByRole('button', { name: 'Cancel' }));
  fireEvent.click(within(confirm).getByRole('button', { name: 'Cancel' }));
  expect(api.disconnect).not.toHaveBeenCalled();
  await sourceAction(selected.name, 'Disconnect…');
  fireEvent.keyDown(within(item).getByRole('button', { name: 'Disconnect' }), { key: 'Escape' });
  expect(within(item).queryByRole('group')).toBeNull(); expect(api.disconnect).not.toHaveBeenCalled();
  await sourceAction(selected.name, 'Disconnect…');
  fireEvent.click(within(item).getByRole('button', { name: 'Disconnect' }));
  await screen.findByText('Selected plans (Google Drive) was disconnected. Its downloaded copies and saved sign-in were removed.');
  expect(api.disconnect).toHaveBeenCalledExactlyOnceWith('actual-service-id');
  expect(changed).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole('listitem', { name: selected.name })).toBeNull();
  expect(api.forget).not.toHaveBeenCalled(); expect(api.disable).not.toHaveBeenCalled(); expect(api.reconnect).not.toHaveBeenCalled();
});
test('Disconnect reports an unconfirmed provider revocation and a shared sign-in honestly', async () => {
  const drive = source('drive-id', 'Work Drive');
  const dropbox: ProviderSource = { ...source('dropbox-id', 'Notes'), kind: 'dropbox', selectedScope: { kind: 'scoped_root', path: '/Notes', recursive: false } };
  const api = backend(state([drive, dropbox]));
  api.disconnect = vi.fn()
    .mockResolvedValueOnce({ credential: 'deleted', revocation: 'failed' })
    .mockResolvedValueOnce({ credential: 'shared', revocation: 'not_attempted' });
  render(<NativeProvidersPanel backend={api} />);
  await sourceAction('Work Drive', 'Disconnect…');
  fireEvent.click(within(screen.getByRole('listitem', { name: 'Work Drive' })).getByRole('button', { name: 'Disconnect' }));
  await screen.findByText(/Google Drive did not confirm that MDReader's access was revoked; you can remove MDReader from your Google Drive account's connected apps\./);
  await waitFor(() => expect((screen.getByRole('button', { name: 'More actions for Notes' }) as HTMLButtonElement).disabled).toBe(false));
  await sourceAction('Notes', 'Disconnect…');
  fireEvent.click(within(screen.getByRole('listitem', { name: 'Notes' })).getByRole('button', { name: 'Disconnect' }));
  await screen.findByText(/Its saved sign-in is still used by another source, so it was kept\./);
});
test('a failed disconnect re-reads status, so the row shows the paused source the service left behind', async () => {
  const selected = source(); const api = backend(state([selected]));
  // As in the service: custody failure happens after the source was paused and its copies forgotten.
  api.disconnect = vi.fn(async () => { selected.enabled = false; selected.health = 'paused'; throw new NativeProviderError({ kind: 'credential_unavailable' }); });
  render(<NativeProvidersPanel backend={api} />);
  await sourceAction('Selected plans', 'Disconnect…');
  fireEvent.click(within(screen.getByRole('listitem', { name: 'Selected plans' })).getByRole('button', { name: 'Disconnect' }));
  expect((await screen.findByRole('alert')).textContent).toBeTruthy();
  const item = screen.getByRole('listitem', { name: 'Selected plans' });
  await within(item).findByText('Paused');
  expect(within(item).queryByText('Connected')).toBeNull();
  expect(within(item).getByRole('button', { name: 'Reconnect' })).toBeTruthy();
  expect(api.state).toHaveBeenCalledTimes(2);
});
test('the More menu is keyboard operable and Escape returns focus to its button', async () => {
  render(<NativeProvidersPanel backend={backend(state([source()]))} />);
  const trigger = await screen.findByRole('button', { name: 'More actions for Selected plans' });
  expect(trigger.getAttribute('aria-haspopup')).toBe('menu');
  fireEvent.click(trigger);
  expect(trigger.getAttribute('aria-expanded')).toBe('true');
  const menu = screen.getByRole('menu', { name: 'More actions for Selected plans' });
  const items = within(menu).getAllByRole('menuitem');
  expect(items.map(item => item.textContent)).toEqual(['Refresh file list', 'Reconnect', 'Pause', 'Disconnect…']);
  expect(document.activeElement).toBe(items[0]);
  fireEvent.keyDown(menu, { key: 'ArrowDown' }); expect(document.activeElement).toBe(items[1]);
  fireEvent.keyDown(menu, { key: 'ArrowUp' }); fireEvent.keyDown(menu, { key: 'ArrowUp' }); expect(document.activeElement).toBe(items[3]);
  const outer = vi.fn();
  document.addEventListener('keydown', outer);
  fireEvent.keyDown(menu, { key: 'Escape' });
  document.removeEventListener('keydown', outer);
  expect(screen.queryByRole('menu')).toBeNull(); expect(document.activeElement).toBe(trigger); expect(outer).not.toHaveBeenCalled();
});
test('the native adapter preserves typed uncertainty and uses only safe command DTOs', async () => {
  raw.invoke.mockResolvedValueOnce({ operation: 'read_cached', projection });
  expect(await nativeProvidersBackend.readCached('actual-source', 'opaque-document')).toEqual(projection);
  expect(raw.invoke).toHaveBeenLastCalledWith('native_provider_request', { request: { operation: 'read_cached', sourceId: 'actual-source', documentId: 'opaque-document' } });
  raw.invoke.mockRejectedValueOnce({ kind: 'unknown_write', operationId: 'uncertain-operation' });
  await expect(nativeProvidersBackend.fetchCurrent('actual-source', 'opaque-document')).rejects.toMatchObject({ detail: { kind: 'unknown_write', operationId: 'uncertain-operation' } });
  raw.invoke.mockRejectedValueOnce('synthetic-private-transport-token');
  const failure = await nativeProvidersBackend.state().catch((error: unknown) => error);
  expect(failure).toBeInstanceOf(NativeProviderError); expect(String(failure)).not.toContain('synthetic-private-transport-token');
});

test('fresh OAuth failure shows safe attempt details without changing its original error message, then clears them', async () => {
  const api = backend();
  const detail = { kind: 'authorization_failed', error: { kind: 'invalid_response' },
    diagnostic: { phase: 'token_request', responseStatus: 400, rejection: 'invalid_client' } };
  api.connect = vi.fn().mockRejectedValueOnce(new NativeProviderError(detail)).mockResolvedValueOnce({ state: 'admitted', sourceId: 'new-source', enabled: true, replayed: false });
  render(<NativeProvidersPanel backend={api} />);
  const form = await addSource('Google Drive');
  fireEvent.click(form.getByRole('button', { name: 'Connect Google Drive' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Google Drive returned a response MDReader could not use.');
  const diagnostic = screen.getByText('Step: token request · HTTP 400 · invalid client');
  fireEvent.click(within(diagnostic.closest('details')!).getByText('Connection details'));
  expect(diagnostic).toBeTruthy();
  expect(providerConnectionDiagnostic(new NativeProviderError({ ...detail, diagnostic: { phase: 'synthetic-private-account', responseStatus: 400, rejection: 'invalid_client' } }))).toBeNull();
  expect(providerConnectionDiagnostic(new NativeProviderError({ ...detail, diagnostic: { phase: 'token_request', responseStatus: 400, rejection: 'synthetic-private-token' } }))).toBeNull();
  await waitFor(() => expect((form.getByRole('button', { name: 'Connect Google Drive' }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(form.getByRole('button', { name: 'Connect Google Drive' }));
  await screen.findByText(/selected source is connected/);
  expect(screen.queryByText('Step: token request · HTTP 400 · invalid client')).toBeNull();
});

test('each connected account renders once with its own status and no first-connection call to action', async () => {
  const drive = source('drive-source', 'Matthew Long');
  const dropbox: ProviderSource = { ...source('dropbox-source', 'Matthew Long'), kind: 'dropbox', enabled: false, health: 'paused', selectedScope: { kind: 'scoped_root', recursive: false } };
  const api = backend(state([drive, dropbox]));
  render(<NativeProvidersPanel backend={api} />);
  await screen.findAllByRole('listitem', { name: 'Matthew Long' });
  expect(screen.getAllByRole('heading', { name: 'Matthew Long' })).toHaveLength(2);
  expect(screen.getAllByText('Matthew Long')).toHaveLength(2);
  const [driveRow, dropboxRow] = screen.getAllByRole('listitem', { name: 'Matthew Long' }).map(item => within(item));
  expect(driveRow!.getByText('Google Drive')).toBeTruthy(); expect(driveRow!.getByText('Connected')).toBeTruthy();
  expect(dropboxRow!.getByText('Dropbox')).toBeTruthy(); expect(dropboxRow!.getByText('Paused')).toBeTruthy();
  expect(screen.queryByText(/Add another/)).toBeNull();
  expect(screen.queryByRole('button', { name: /^Connect / })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Manage cloud sources' })).toBeNull();
  expect(api.connect).not.toHaveBeenCalled(); expect(api.reconnect).not.toHaveBeenCalled();
});
