import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../../App';
import { catalogBackend, type CatalogDocument, type CatalogSource } from './backend';
import { deviceReaderBackend } from '../device-reader/backend';
import { nativeProvidersBackend } from '../../platform/providers';
import { useTabsStore } from '../../stores/tabs';
import { useSourcesStore } from '../../stores/sources';
import { useSettingsStore } from '../../stores/settings';
import { useEditorStore } from '../../stores/editor';
import type { HydratedLibrarySelection } from '../../shell/library-documents';
import { legacyEvidence, legacySelection } from '../legacy-records/test-fixture';

const legacy = vi.hoisted(() => ({ hydrate: vi.fn(), read: vi.fn() }));
vi.mock('../../shell/library-documents', async (original) => ({
  ...await original<typeof import('../../shell/library-documents')>(),
  hydrateLibrary: legacy.hydrate,
  hydrateLibraryDocument: legacy.read,
}));
vi.mock('../../shell/persistence', async (original) => ({
  ...await original<typeof import('../../shell/persistence')>(),
  isTauri: () => true,
  loadPersisted: async () => ({ session: null, settings: {} }),
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
  legacy.hydrate.mockResolvedValue({ evidence: [legacyEvidence], failures: [] });
  legacy.read.mockResolvedValue(legacySelection);
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
  legacy.hydrate.mockReset();
  legacy.read.mockReset();
  useTabsStore.getState().reset();
  useSourcesStore.getState().reset();
  useSettingsStore.getState().reset();
  useEditorStore.getState().reset();
});
async function openCurrent() {
  fireEvent.click(await screen.findByRole('button', { name: 'Browse catalog Retained notes' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Read exact Current note' }));
  return screen.findByRole('heading', { name: /Canonical content/ });
}

test('primary startup lists canonical sources only and reads the exact selection in the normal main reader', async () => {
  render(<App />);
  await screen.findByRole('button', { name: 'Browse catalog Retained notes' });
  expect(legacy.hydrate).not.toHaveBeenCalled();
  expect(legacy.read).not.toHaveBeenCalled();
  await openCurrent();
  expect(within(screen.getByRole('main')).getByRole('heading', { name: /Canonical content/ })).toBeTruthy();
  expect(screen.queryByRole('dialog', { name: 'Catalog document' })).toBeNull();
  expect(documentElement('.shell__reader-content').hasAttribute('inert')).toBe(false);
  expect(catalogBackend.readDocument).toHaveBeenCalledWith({ sourceId, path: 'current.md', revision, title: 'Current note', line: 1 });
  expect(legacy.read).not.toHaveBeenCalled();
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
  fireEvent.click(await screen.findByRole('button', { name: 'Browse catalog On this device' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Read exact Received copy' }));
  expect(await within(screen.getByRole('main')).findByRole('heading', { name: /Received device copy/ })).toBeTruthy();
  expect(read).toHaveBeenCalledWith({ documentId: deviceId, sourceId, path: 'document.md', revision, title: 'Received copy', line: 1 });
  expect(catalogBackend.readDocument).not.toHaveBeenCalled();
  expect(legacy.read).not.toHaveBeenCalled();
  expect(screen.queryByRole('dialog', { name: 'Device document' })).toBeNull();
});
function documentElement(selector: string): Element {
  const element = window.document.querySelector(selector);
  if (!element) throw new Error(`Missing element ${selector}`);
  return element;
}

test('explicit legacy read remains available but its late body cannot replace a newer canonical selection', async () => {
  let finish!: (value: HydratedLibrarySelection) => void;
  legacy.read.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  render(<App />);
  fireEvent.click(screen.getByRole('button', { name: 'Legacy records (migration)' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Open Historical note from Historical account' }));
  await waitFor(() => expect(legacy.read).toHaveBeenCalledOnce());
  await openCurrent();
  await act(async () => finish(legacySelection));
  expect(screen.getByRole('heading', { name: /Canonical content/ })).toBeTruthy();
  expect(screen.queryByRole('heading', { name: /Historical body/ })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Close legacy records' })).toBeNull();
});

test('a preserved historical Recent explicitly invokes the legacy adapter with its existing identity and edit receipt', async () => {
  render(<App />);
  await screen.findByRole('button', { name: 'Browse catalog Retained notes' });
  act(() => useSourcesStore.getState().addRecent({
    kind: 'cloud', sourceId: legacySelection.member.registry_id, recordKey: 'historical-key',
    documentKey: 'historical-document-key', name: 'Historical recent',
  }, null));
  fireEvent.click(screen.getByRole('button', { name: 'Historical recent' }));
  expect(await screen.findByRole('heading', { name: /Historical body/ })).toBeTruthy();
  expect(legacy.hydrate).toHaveBeenCalledOnce();
  expect(legacy.read).toHaveBeenCalledWith(expect.anything(), expect.objectContaining({
    member: legacySelection.member, row: legacySelection.row,
  }), expect.stringContaining('library-document:'));
  expect(useTabsStore.getState().tabs[0]?.docRef).toEqual({
    kind: 'cloud', sourceId: legacySelection.member.registry_id, recordKey: 'historical-key',
    documentKey: 'historical-document-key', name: 'Historical note',
  });
  expect(screen.getByRole('button', { name: 'Source view' })).toBeTruthy();
  expect(catalogBackend.readDocument).not.toHaveBeenCalled();
});
