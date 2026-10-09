import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../../App';
import { catalogBackend, type CatalogDocument, type CatalogSource } from './backend';
import { deviceReaderBackend } from '../device-reader/backend';
import { archivedReviewsBackend, type ArchivedReviewRecord, type ArchivedReviewComparison } from '../../reviews/adapter';
import { nativeProvidersBackend } from '../../platform/providers';
import { detectMac } from '../../keymap/registry';
import { restoreReaderHistory } from '../reader-history/serialization';
import * as filesystem from '../../platform/fs';
import * as settingsPlatform from '../../platform/settings';
import { useReaderHistoryStore } from '../reader-history/store';
import { useTabsStore } from '../../stores/tabs';
import { useSourcesStore } from '../../stores/sources';
import { useSettingsStore } from '../../stores/settings';
import { useEditorStore } from '../../stores/editor';
const legacy = vi.hoisted(() => ({ load: vi.fn() }));
vi.mock('../../shell/persistence', async (original) => ({
  ...await original<typeof import('../../shell/persistence')>(),
  isTauri: () => true,
  loadPersisted: legacy.load,
  saveSettings: async () => {},
}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ onCloseRequested: async () => () => {}, destroy: async () => {} }),
}));
const sourceId = 'a'.repeat(64);
const revision = 'b'.repeat(64);
const source: CatalogSource = {
  catalogId: `local:${sourceId}`, name: 'Retained notes', kind: 'local', enabled: true,
  coverage: 'indexed_observation', observedAtMs: 1, failure: null, librarySourceId: 'retained-library-id',
};
const document: CatalogDocument = {
  name: 'Current note', displayPath: 'current.md', modifiedAtMs: 1, sizeBytes: null,
  observedAtMs: 1, observation: 'indexed_local',
  open: { kind: 'exact', sourceId, path: 'current.md', revision },
};

beforeEach(() => {
  legacy.load.mockResolvedValue({ session: null, settings: {}, readerHistory: restoreReaderHistory(undefined, true) });
  vi.spyOn(nativeProvidersBackend, 'state').mockResolvedValue({ availability: [], sources: [], pendingAdmissions: [], error: null });
  vi.spyOn(catalogBackend, 'list').mockResolvedValue({ operation: 'list', sources: [source], providerFailure: null, localFailure: null });
  vi.spyOn(catalogBackend, 'page').mockResolvedValue({ operation: 'page', source, documents: [document], nextCursor: null });
  vi.spyOn(catalogBackend, 'close').mockResolvedValue(undefined);
  vi.spyOn(catalogBackend, 'readDocument').mockResolvedValue({
    operation: 'read_document', sourceId, path: 'current.md', revision,
    markdown: '# Canonical content\n\nExact current revision.', lineStart: 1, lineEnd: 3,
  });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  useReaderHistoryStore.getState().reset();
  useTabsStore.getState().reset();
  useSourcesStore.getState().reset();
  useSettingsStore.getState().reset();
  useEditorStore.getState().reset();
});
async function openCurrent() {
  fireEvent.click(await screen.findByRole('button', { name: 'Browse Retained notes' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Open Current note' }));
  return screen.findByRole('heading', { name: /Canonical content/ });
}

test('primary startup lists canonical sources only and reads the exact selection in the normal main reader', async () => {
  render(<App />);
  await screen.findByRole('button', { name: 'Browse Retained notes' });
  await openCurrent();
  expect(within(screen.getByRole('main')).getByRole('heading', { name: /Canonical content/ })).toBeTruthy();
  expect(screen.queryByRole('dialog', { name: 'Catalog document' })).toBeNull();
  expect(documentElement('.shell__reader-content').hasAttribute('inert')).toBe(false);
  expect(catalogBackend.readDocument).toHaveBeenCalledWith({ sourceId, path: 'current.md', revision, title: 'Current note', line: 1 });
});

test('published device receipts open inline through the service device reader without local folder or legacy reads', async () => {
  const deviceId = '00000000-0000-4000-8000-000000000001';
  const deviceSource = { ...source, catalogId: 'device:library', name: 'On this device', kind: 'device', coverage: 'published_device_metadata', librarySourceId: null };
  vi.mocked(catalogBackend.list).mockResolvedValue({ operation: 'list', sources: [deviceSource], providerFailure: null, localFailure: null });
  vi.mocked(catalogBackend.page).mockResolvedValue({ operation: 'page', source: deviceSource, nextCursor: null, documents: [{ ...document,
    name: 'Received copy', displayPath: 'received.md', observation: 'published_device', open: { kind: 'device', documentId: deviceId, sourceId, revision, line: 1 } }] });
  const read = vi.spyOn(deviceReaderBackend, 'read').mockResolvedValue({ operation: 'read_document', documentId: deviceId, sourceId, path: 'document.md', revision, markdown: '# Received device copy', lineStart: 1, lineEnd: 1 });
  vi.spyOn(deviceReaderBackend, 'manifest').mockResolvedValue(null);
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Browse On this device' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Open Received copy' }));
  expect(await within(screen.getByRole('main')).findByRole('heading', { name: /Received device copy/ })).toBeTruthy();
  expect(read).toHaveBeenCalledWith({ documentId: deviceId, sourceId, path: 'document.md', revision, title: 'Received copy', line: 1 });
  expect(catalogBackend.readDocument).not.toHaveBeenCalled();
  expect(screen.queryByRole('dialog', { name: 'Device document' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Archived reviews…' })).toBeNull();
});
function documentElement(selector: string): Element {
  const element = window.document.querySelector(selector);
  if (!element) throw new Error(`Missing element ${selector}`);
  return element;
}

test('historical Recent keeps its identity without fetching, regranting or opening developer migration tools', async () => {
  const providerRead = vi.spyOn(nativeProvidersBackend, 'readDocument');
  const fileRead = vi.spyOn(filesystem, 'readDoc');
  const reference = { kind: 'cloud' as const, sourceId: 'historical-source', recordKey: 'historical-key',
    documentKey: 'historical-document-key', name: 'Historical recent' };
  render(<App />);
  await screen.findByRole('button', { name: 'Browse Retained notes' });
  act(() => useSourcesStore.getState().addRecent(reference, null));
  fireEvent.click(screen.getByRole('button', { name: 'Historical recent' }));
  expect(await screen.findByRole('heading', { name: 'Historical recent' })).toBeTruthy();
  expect(useTabsStore.getState().tabs[0]?.docRef).toEqual(reference);
  expect(providerRead).not.toHaveBeenCalled();
  expect(fileRead).not.toHaveBeenCalled();
  expect(catalogBackend.readDocument).not.toHaveBeenCalled();
  expect(within(screen.getByRole('region', { name: 'Retained historical document' }))
    .queryByRole('button', { name: 'Migration & archived records' })).toBeNull();
  expect(screen.queryByRole('dialog', { name: 'Settings' })).toBeNull();
  expect(useTabsStore.getState().tabs[0]?.docRef).toEqual(reference);
});

const archivalRow: ArchivedReviewRecord = {
  recordId: 'd'.repeat(64), rowId: '1', artifactIndex: 0, rowSha256: 'e'.repeat(64),
  state: 'rejected', reviewer: null, recordedContentHash: null, reviewedContentHash: null,
  recordedHashComparison: 'unknown', graphMembership: 'unavailable', chronology: 'unavailable',
  schema: 'decision_overlay', extractor: 'mdreader.decision@1', rule: 'D1', qualified: 'Decision',
  reading: 'Recorded decision', startLine: 1, endLine: 1, unavailableReason: null,
};
function archiveBoundary() {
  vi.spyOn(archivedReviewsBackend, 'select').mockResolvedValue({
    handle: 'selected-archive', exportId: 'export', manifestSha256: 'hash', ownerScope: 'profile',
  });
  vi.spyOn(archivedReviewsBackend, 'list').mockResolvedValue({ records: [archivalRow], nextOffset: null });
  vi.spyOn(archivedReviewsBackend, 'current');
  vi.spyOn(archivedReviewsBackend, 'close').mockResolvedValue(undefined);
}
async function openArchive() {
  fireEvent.click(screen.getByRole('button', { name: 'Archived reviews…' }));
  fireEvent.change(screen.getByLabelText('Export UUID'), { target: { value: '11111111-1111-4111-8111-111111111111' } });
  fireEvent.change(screen.getByLabelText('Manifest SHA-256'), { target: { value: 'a'.repeat(64) } });
  fireEvent.click(screen.getByRole('button', { name: 'Choose review stage…' }));
  await screen.findByText('rejected archival ruling');
}
test.each(['local', 'provider'] as const)('canonical %s archive comparison uses the navigated exact receipt and returns inline without reloading', async (kind) => {
  archiveBoundary();
  const selectedSourceId = kind === 'provider' ? `provider:${sourceId}` : sourceId;
  const selectedSource = { ...source, kind };
  const origin = { sourceId: selectedSourceId, path: kind === 'provider' ? `records/${'1'.repeat(64)}.md` : 'current.md', revision, title: 'Current note', line: 1 };
  const linked = { ...origin, path: kind === 'provider' ? `records/${'2'.repeat(64)}.md` : 'linked.md', revision: 'f'.repeat(64), title: 'Linked note' };
  vi.mocked(catalogBackend.page).mockResolvedValue({ operation: 'page', source: selectedSource, nextCursor: null,
    documents: [{ ...document, open: { kind: 'exact', sourceId: selectedSourceId, path: origin.path, revision } }] });
  vi.mocked(catalogBackend.list).mockResolvedValue({ operation: 'list', sources: [selectedSource], providerFailure: null, localFailure: null });
  const backend = kind === 'provider' ? nativeProvidersBackend : catalogBackend;
  const read = kind === 'provider' ? vi.spyOn(nativeProvidersBackend, 'readDocument') : vi.mocked(catalogBackend.readDocument);
  read.mockImplementation(async (reference) => ({ operation: 'read_document', ...reference,
    markdown: reference.path === origin.path ? '# Canonical content\n\n[Next](linked.md)' : '# Linked exact copy', lineStart: 1, lineEnd: 3 }));
  vi.spyOn(backend, 'resolveLink').mockResolvedValue(kind === 'provider' ? {
    state: 'resolved', projection: { jobId: 'cached-job', readerSourceId: selectedSourceId, path: linked.path, revision: linked.revision },
    candidate: { sourceId: 'service-source', documentId: 'selected-document', title: linked.title }, line: 1, heading: null,
  } : { state: 'local_matches', references: [linked] });
  const exact = { sourceId: selectedSourceId, path: linked.path, revision: linked.revision };
  vi.spyOn(archivedReviewsBackend, 'compare').mockResolvedValue({ review: archivalRow, current: exact,
    observation: kind === 'provider' ? 'provider_cached_revision' : 'local_current_read', currentSpanHash: 'blake3:span',
    currentPassage: 'Exact compared passage', recordedCurrent: 'unknown', reviewedCurrent: 'unknown' });
  render(<App />);
  await openCurrent();
  fireEvent.click(screen.getByRole('link', { name: 'Next' }));
  await screen.findByRole('heading', { name: /Linked exact copy/ });
  await openArchive();
  expect(documentElement('.shell__reader-content').hasAttribute('inert')).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Compare with open document' }));
  await screen.findByLabelText('Current revision comparison');
  expect(archivedReviewsBackend.compare).toHaveBeenCalledWith('selected-archive', archivalRow.recordId, exact, null);
  expect(archivedReviewsBackend.current).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Close archived reviews' }));
  expect(screen.queryByRole('dialog', { name: 'Archived document reviews' })).toBeNull();
  expect(within(screen.getByRole('main')).getByRole('heading', { name: /Linked exact copy/ })).toBeTruthy();
  expect(read).toHaveBeenCalledTimes(2);
  expect(archivedReviewsBackend.close).toHaveBeenCalledWith('selected-archive');
  fireEvent.click(screen.getByRole('button', { name: 'Previous document' }));
  await screen.findByRole('heading', { name: /Canonical content/ });
});
test('a new canonical selection closes its archive and a late comparison cannot restore the old reader', async () => {
  archiveBoundary();
  let finish!: (value: ArchivedReviewComparison) => void;
  vi.spyOn(archivedReviewsBackend, 'compare').mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  vi.mocked(catalogBackend.page).mockResolvedValue({ operation: 'page', source, nextCursor: null,
    documents: [document, { ...document, name: 'New selection', open: { kind: 'exact', sourceId, path: 'new.md', revision: 'f'.repeat(64) } }] });
  vi.mocked(catalogBackend.readDocument).mockImplementation(async (reference) => ({ operation: 'read_document', ...reference,
    markdown: reference.path === 'new.md' ? '# New exact selection' : '# Canonical content', lineStart: 1, lineEnd: 1 }));
  render(<App />);
  await openCurrent();
  const replacement = screen.getByRole('button', { name: 'Open New selection' });
  await openArchive();
  fireEvent.click(screen.getByRole('button', { name: 'Compare with open document' }));
  await waitFor(() => expect(archivedReviewsBackend.compare).toHaveBeenCalledOnce());
  // Model a source/selection replacement while asynchronous archive work is pending.
  fireEvent.click(replacement);
  await screen.findByRole('heading', { name: /New exact selection/ });
  expect(screen.queryByRole('dialog', { name: 'Archived document reviews' })).toBeNull();
  expect(archivedReviewsBackend.close).toHaveBeenCalledWith('selected-archive');
  await act(async () => finish({ review: archivalRow, current: { sourceId, path: 'current.md', revision },
    observation: 'local_current_read', currentSpanHash: 'blake3:span', currentPassage: 'Old late passage', recordedCurrent: 'unknown', reviewedCurrent: 'unknown' }));
  expect(screen.queryByText('Old late passage')).toBeNull();
  expect(screen.getByRole('heading', { name: /New exact selection/ })).toBeTruthy();
});


test('restored canonical tabs retain unavailable exact receipts without reopening old folders or falling back to latest', async () => {
  const store = useReaderHistoryStore.getState();
  store.open({ kind: 'local', sourceId, path: 'saved.md', revision }, 'Saved exact copy');
  const saved = JSON.parse(JSON.stringify(useReaderHistoryStore.getState().data));
  store.reset();
  legacy.load.mockResolvedValue({ settings: {}, readerHistory: restoreReaderHistory(saved, true),
    session: { root: '/previously-selected', activeTabId: null, tabs: [], recent: [] } });
  const reconcile = vi.spyOn(settingsPlatform, 'reconcileLibraryRestore');
  const filesystemRead = vi.spyOn(filesystem, 'readDoc');
  vi.mocked(catalogBackend.readDocument).mockRejectedValue(new Error('The exact saved revision is unavailable.'));
  render(<App />);
  expect(await within(screen.getByRole('main')).findByRole('alert')).toHaveProperty('textContent', 'The exact saved revision is unavailable.');
  expect(screen.getByRole('tab', { name: /Saved exact copy/ }).getAttribute('aria-selected')).toBe('true');
  expect(useReaderHistoryStore.getState().data.tabs[0]!.entries[0]!.reference).toEqual(saved.tabs[0].entries[0].reference);
  expect(catalogBackend.readDocument).toHaveBeenCalledOnce();
  expect(catalogBackend.readDocument).toHaveBeenCalledWith({ sourceId, path: 'saved.md', revision, title: 'Saved exact copy', line: 1 });
  expect(reconcile).not.toHaveBeenCalled();
  expect(filesystemRead).not.toHaveBeenCalled();
  const mac = detectMac();
  fireEvent.keyDown(window, { key: 'w', metaKey: mac, ctrlKey: !mac });
  expect(screen.queryByRole('tab', { name: /Saved exact copy/ })).toBeNull();
  expect(screen.getByRole('button', { name: 'Saved exact copy' })).toBeTruthy();
  fireEvent.keyDown(window, { key: 'T', shiftKey: true, metaKey: mac, ctrlKey: !mac });
  expect(await screen.findByRole('tab', { name: /Saved exact copy/ })).toBeTruthy();
  expect(filesystemRead).not.toHaveBeenCalled();
  expect(reconcile).not.toHaveBeenCalled();
});


test('connecting an empty Dropbox source refreshes Library and exposes browsing without downloading bodies', async () => {
  let connected = false;
  const cloud: CatalogSource = {
    catalogId: 'remote:connected-dropbox', name: 'Dropbox notes', kind: 'dropbox',
    enabled: true, coverage: 'never_indexed', observedAtMs: null,
    failure: null, librarySourceId: null,
  };
  const summary = {
    sourceId: 'connected-dropbox', readerSourceId: `provider:${'c'.repeat(64)}`,
    name: cloud.name, kind: 'dropbox' as const, enabled: true,
    selectedScope: { kind: 'scoped_root' as const, recursive: false },
    capabilities: { read: true, list: true, changes: true, assets: false, guardedWrite: false },
    health: 'ready' as const, coverage: { kind: 'never_indexed' as const },
  };
  vi.mocked(nativeProvidersBackend.state).mockImplementation(async () => ({
    availability: [{ provider: 'dropbox', configured: true, supported: true, diagnostic: null }],
    sources: connected ? [summary] : [], pendingAdmissions: [], error: null,
  }));
  const connect = vi.spyOn(nativeProvidersBackend, 'connect').mockImplementation(async () => {
    connected = true;
    return { state: 'admitted', sourceId: summary.sourceId, enabled: true, replayed: false };
  });
  const refresh = vi.spyOn(nativeProvidersBackend, 'refresh').mockResolvedValue({ operation: 'refresh', pages: 1, metadataComplete: true });
  const fetch = vi.spyOn(nativeProvidersBackend, 'fetchCurrent');
  vi.mocked(catalogBackend.page)
    .mockResolvedValueOnce({ operation: 'page', source: cloud, documents: [], nextCursor: null })
    .mockResolvedValue({ operation: 'page', source: { ...cloud, coverage: 'selected_metadata' }, documents: [], nextCursor: null });
  render(<App />);
  await screen.findByRole('button', { name: 'Browse Retained notes' });
  fireEvent.click(screen.getByRole('button', { name: 'Add source' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Manage cloud sources' }));
  const connectButton = await screen.findByRole('button', { name: 'Connect Dropbox folder' });
  await waitFor(() => expect((connectButton as HTMLButtonElement).disabled).toBe(false));
  const listCalls = vi.mocked(catalogBackend.list).mock.calls.length;
  fireEvent.click(connectButton);
  await screen.findByText('The selected source is connected. Refresh its metadata to browse documents.');
  await waitFor(() => expect(vi.mocked(catalogBackend.list).mock.calls.length).toBeGreaterThan(listCalls));
  fireEvent.click(screen.getByRole('button', { name: 'Close Settings' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Browse Dropbox notes' }));
  await screen.findByText('No Markdown documents found in your selected cloud source.');
  expect(catalogBackend.page).toHaveBeenCalledWith(cloud.catalogId, null, 100);
  expect(refresh).toHaveBeenCalledExactlyOnceWith(summary.sourceId);
  expect(connect).toHaveBeenCalledWith('dropbox', { path: '/', recursive: false }, false);
  expect(fetch).not.toHaveBeenCalled();
});
